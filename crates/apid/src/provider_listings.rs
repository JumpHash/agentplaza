//! The providers' model listings (PLAN 30.4): the last good listing of each provider, served at once and refreshed in
//! the background.
//!
//! - A listing is never fetched on a request that can be answered from the cache: `GET /v1/models` answers with what
//!   the cache holds (stale or not) and only a provider never listed is waited for, briefly, on a cold start.
//! - Each provider has a generation, bumped with every change to it (added, edited, removed, enabled, disabled, its key
//!   set). A refresh stores its result only if the provider's generation did not move while it ran, so a refresh that
//!   started before an edit never overwrites it; concurrent refreshes of one provider share one fetch.
//! - The refresher lists every provider whose listing is missing, older than its interval or from an older
//!   generation, all at once, each under the listing client's own deadline (no retries, `LISTING_DEADLINE`).
//!   A provider's interval is `options.models_cache_secs` where it sets one (OpenRouter: an hour), else [`REFRESH`].
//! - The health checks (`provider_health.rs`) refresh the listing they probe; they no longer clear anything.
//! - The listings are kept in the hub's store (component `provider_listings`), so a restart starts warm.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use apz_cache::{Cache, Generations};
use apz_gateway::{Caller, Gateway, ModelInfo, ProviderConfig};
use serde::{Deserialize, Serialize};

/// How often a provider is listed again when it sets no interval of its own.
pub const REFRESH: Duration = Duration::from_secs(15 * 60);
/// How long `GET /v1/models` waits on a cold start for providers never listed yet.
pub const COLD_WAIT: Duration = Duration::from_millis(1500);
/// How often the refresher looks for listings that are due.
pub const TICK: Duration = Duration::from_secs(30);

/// A provider's model listing as last fetched (a row of `provider_listing` and its models).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "provider_listing")]
pub struct Listing {
    #[table(key)]
    pub provider: String,
    pub fetched_ms: u64,
    #[table(child)]
    pub models: Vec<ListedModel>,
}

/// One model of a listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct ListedModel {
    pub id: String,
    pub name: Option<String>,
    pub context: Option<u64>,
}

impl From<ModelInfo> for ListedModel {
    fn from(m: ModelInfo) -> Self {
        ListedModel {
            id: m.id,
            name: m.name,
            context: m.context,
        }
    }
}

impl From<&ListedModel> for ModelInfo {
    fn from(m: &ListedModel) -> Self {
        let mut info = ModelInfo::new(m.id.clone());
        info.name.clone_from(&m.name);
        info.context = m.context;
        info
    }
}

/// A provider's last listing attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attempt {
    pub at_ms: u64,
    pub latency_ms: u64,
    /// Why it failed; none when it succeeded.
    pub error: Option<String>,
}

/// The listings of every provider.
pub struct Listings {
    cache: Arc<Cache<String, Listing>>,
    generations: Arc<Generations<String>>,
    attempts: Mutex<BTreeMap<String, Attempt>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for Listings {
    fn default() -> Self {
        Listings::new()
    }
}

impl Listings {
    pub fn new() -> Listings {
        Listings {
            // An entry is listed again on its interval by the refresher; the cache's own age bound is a backstop.
            cache: Arc::new(Cache::new(Duration::from_secs(24 * 3600))),
            generations: Arc::new(Generations::new()),
            attempts: Mutex::new(BTreeMap::new()),
        }
    }

    /// The provider's generation now.
    pub fn generation(&self, id: &str) -> u64 {
        self.generations.get(&id.to_string())
    }

    /// Marks provider `id` changed: its listing is from an older generation and is refreshed.
    pub fn changed(&self, id: &str) {
        self.generations.bump(&id.to_string());
    }

    /// Forgets a removed provider.
    pub fn removed(&self, id: &str) {
        self.generations.bump(&id.to_string());
        self.cache.remove(&id.to_string());
        lock(&self.attempts).remove(id);
    }

    /// The last good listing of `id`, however old, without fetching.
    pub fn peek(&self, id: &str) -> Option<Arc<Listing>> {
        self.cache.peek(&id.to_string()).map(|e| e.value)
    }

    /// The last listing attempt of `id`.
    pub fn attempt(&self, id: &str) -> Option<Attempt> {
        lock(&self.attempts).get(id).cloned()
    }

    /// Lists `p` now (forced: even a fresh listing is fetched again) or only when its listing is missing or stale.
    /// The fetch uses the gateway's listing client: one deadline, no retries.
    pub fn fetch(&self, gateway: &Gateway, p: &ProviderConfig, force: bool) -> Result<Arc<Listing>, String> {
        let id = p.id.clone();
        let generations = Arc::clone(&self.generations);
        let current = {
            let id = id.clone();
            move || generations.get(&id)
        };
        let started = Instant::now();
        let build = || -> Result<Listing, String> {
            let models = gateway
                .list_models_for(&id, &Caller::Owner { node: None })
                .map_err(|e| e.to_string())?;
            Ok(Listing {
                provider: id.clone(),
                fetched_ms: apz_plaza::now_ms(),
                models: models.into_iter().map(ListedModel::from).collect(),
            })
        };
        let result = if force {
            self.cache.refresh(&id, current, build)
        } else {
            self.cache.get(&id, current, build)
        };
        lock(&self.attempts).insert(
            id,
            Attempt {
                at_ms: apz_plaza::now_ms(),
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                error: result.as_ref().err().cloned(),
            },
        );
        result
    }

    /// The providers of `providers` (enabled ones) whose listing is due: missing, from an older generation, or older
    /// than their interval.
    pub fn due<'a>(&self, providers: &'a [ProviderConfig]) -> Vec<&'a ProviderConfig> {
        let now = apz_plaza::now_ms();
        providers
            .iter()
            .filter(|p| p.enabled)
            .filter(|p| match self.cache.peek(&p.id) {
                None => true,
                Some(entry) => {
                    entry.generation != self.generation(&p.id)
                        || now.saturating_sub(entry.value.fetched_ms) >= interval_of(p).as_millis() as u64
                }
            })
            .collect()
    }

    /// Lists every provider of `providers` at once (each on its own thread, under its own deadline) and waits at most
    /// `wait` for them; a listing that finishes later still lands in the cache. Returns the ids that finished.
    pub fn fetch_all(
        self: &Arc<Self>,
        gateway: &Gateway,
        providers: &[&ProviderConfig],
        force: bool,
        wait: Duration,
    ) -> Vec<String> {
        let (done_tx, done_rx) = mpsc::channel();
        for p in providers {
            let (me, gateway, p, done) = (Arc::clone(self), gateway.clone(), (*p).clone(), done_tx.clone());
            std::thread::Builder::new()
                .name(format!("listing-{}", p.id))
                .spawn(move || {
                    let _ = me.fetch(&gateway, &p, force);
                    let _ = done.send(p.id);
                })
                .ok();
        }
        drop(done_tx);
        let deadline = Instant::now() + wait;
        let mut finished = Vec::new();
        while finished.len() < providers.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            match done_rx.recv_timeout(left) {
                Ok(id) => finished.push(id),
                Err(_) => break,
            }
        }
        finished
    }

    /// Every listing, for the hub's store.
    pub fn rows(&self, providers: &[ProviderConfig]) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let listings: Vec<Arc<Listing>> = providers.iter().filter_map(|p| self.peek(&p.id)).collect();
        apz_store::RowSet::list(def(), listings.iter().map(|l| &**l))
    }

    /// Fills the cache from the hub's store (at start), each listing at its provider's generation now.
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), apz_store::StoreError> {
        for listing in apz_store::table::load_list::<Listing>(conn, def())? {
            let generation = self.generation(&listing.provider);
            self.cache.put(listing.provider.clone(), listing, generation);
        }
        Ok(())
    }
}

/// The listings' tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Listing)
}

/// How often `p` is listed again: its `options.models_cache_secs` when it sets one, else [`REFRESH`].
pub fn interval_of(p: &ProviderConfig) -> Duration {
    p.options
        .get("models_cache_secs")
        .and_then(serde_json::Value::as_u64)
        .filter(|s| *s > 0)
        .map_or(REFRESH, Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_gateway::ProviderKind;

    fn mock(id: &str) -> ProviderConfig {
        ProviderConfig::new(id, ProviderKind::Mock)
    }

    #[test]
    fn a_listing_is_kept_and_a_change_makes_it_due() {
        let p = mock("m");
        let gateway = Gateway::builder().providers(vec![p.clone()]).build();
        let listings = Listings::new();
        assert_eq!(listings.due(std::slice::from_ref(&p)).len(), 1, "never listed");
        let first = listings.fetch(&gateway, &p, false).unwrap();
        assert!(!first.models.is_empty());
        assert!(listings.due(std::slice::from_ref(&p)).is_empty(), "fresh");
        listings.changed("m");
        assert_eq!(listings.due(std::slice::from_ref(&p)).len(), 1, "a change makes it due");
        assert_eq!(listings.attempt("m").unwrap().error, None);
    }

    #[test]
    fn listings_round_trip_through_the_store() {
        let p = mock("m");
        let gateway = Gateway::builder().providers(vec![p.clone()]).build();
        let listings = Listings::new();
        listings.fetch(&gateway, &p, false).unwrap();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &listings.rows(std::slice::from_ref(&p)).unwrap()).unwrap();
        let restored = Listings::new();
        restored.load(&conn).unwrap();
        assert_eq!(restored.peek("m"), listings.peek("m"));
    }

    #[test]
    fn an_interval_comes_from_the_provider_options() {
        let mut p = mock("o");
        assert_eq!(interval_of(&p), REFRESH);
        p.options = serde_json::json!({ "models_cache_secs": 3600 });
        assert_eq!(interval_of(&p), Duration::from_secs(3600));
    }
}
