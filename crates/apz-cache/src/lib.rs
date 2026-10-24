//! Caches kept true by generations, not by events (PLAN 30.4).
//!
//! - **Generations.** Every source of cached data (a provider, a role, the settings) has a generation number, bumped
//!   under the same lock as the write that changes it ([`Generations::bump`]). A cache entry records the generation it
//!   was built from and is answered only while that is still the source's generation; events are hints that let a
//!   cache refresh early, never the guarantee (the hub's bus drops a listener whose queue fills).
//! - **Refreshes cannot undo writes.** A build notes the generation when it starts and its result is stored only if
//!   the generation is unchanged when it ends; otherwise the build runs again ([`Cache::get`]). Builds are
//!   single-flight per key: concurrent misses wait for one build.
//! - **A maximum age** backs every entry up.
//! - **Stale while revalidate** ([`Cache::get_or_refresh`]): for upstream data, the last good value is answered at
//!   once and a refresh runs in the background.
//! - **ETags** over a response as sent ([`etag`], [`not_modified`]).

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The generations of a family of sources, by key, plus one for the family as a whole (bumped when every source of
/// the family changes at once, a restore for example). A key never seen has generation 0.
#[derive(Debug, Default)]
pub struct Generations<K> {
    keys: RwLock<HashMap<K, Arc<AtomicU64>>>,
    all: AtomicU64,
}

impl<K: Hash + Eq + Clone> Generations<K> {
    pub fn new() -> Self {
        Generations {
            keys: RwLock::new(HashMap::new()),
            all: AtomicU64::new(0),
        }
    }

    fn slot(&self, key: &K) -> Arc<AtomicU64> {
        if let Some(slot) = self
            .keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Arc::clone(slot);
        }
        let mut keys = self.keys.write().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(keys.entry(key.clone()).or_default())
    }

    /// The source's generation now: its own and the family's, combined so a bump of either changes it.
    pub fn get(&self, key: &K) -> u64 {
        let own = self.slot(key).load(Ordering::Acquire);
        own.wrapping_add(self.all.load(Ordering::Acquire) << 32)
    }

    /// Marks the source changed. Call it inside the lock (or transaction) of the write that changes it.
    pub fn bump(&self, key: &K) -> u64 {
        self.slot(key).fetch_add(1, Ordering::AcqRel);
        self.get(key)
    }

    /// Marks every source of the family changed.
    pub fn bump_all(&self) {
        self.all.fetch_add(1, Ordering::AcqRel);
    }
}

/// A cached value: what it was built from and when.
#[derive(Debug)]
pub struct Entry<V> {
    pub value: Arc<V>,
    pub generation: u64,
    pub built: Instant,
}

impl<V> Clone for Entry<V> {
    fn clone(&self) -> Self {
        Entry {
            value: Arc::clone(&self.value),
            generation: self.generation,
            built: self.built,
        }
    }
}

/// A build in flight: who waits for it, and its outcome once it has one.
struct Flight<V> {
    generation: u64,
    done: Mutex<Option<Result<Arc<V>, String>>>,
    ready: Condvar,
}

enum Slot<V> {
    Ready(Entry<V>),
    Building {
        flight: Arc<Flight<V>>,
        last: Option<Entry<V>>,
    },
}

/// A cache of values by key. See the crate docs.
pub struct Cache<K, V> {
    slots: Mutex<HashMap<K, Slot<V>>>,
    max_age: Duration,
    hits: AtomicU64,
    builds: AtomicU64,
}

/// How many times a build is retried when its source changed while it ran, before its result is answered unstored.
const REBUILDS: usize = 3;

impl<K: Hash + Eq + Clone + Send + 'static, V: Send + Sync + 'static> Cache<K, V> {
    /// A cache whose entries are rebuilt past `max_age` even when their source did not change.
    pub fn new(max_age: Duration) -> Self {
        Cache {
            slots: Mutex::new(HashMap::new()),
            max_age,
            hits: AtomicU64::new(0),
            builds: AtomicU64::new(0),
        }
    }

    fn fresh(&self, entry: &Entry<V>, generation: u64) -> bool {
        entry.generation == generation && entry.built.elapsed() < self.max_age
    }

    /// The value for `key` at its source's generation now (`current` reads it): a fresh entry is answered as it is;
    /// otherwise `build` runs once for every caller that misses at this generation, and its result is stored only if
    /// the generation did not move while it ran (else it builds again, at most a few times). A failed build is not
    /// stored; its error goes to every caller that waited for it.
    pub fn get<E: std::fmt::Display>(
        &self,
        key: &K,
        current: impl Fn() -> u64,
        build: impl Fn() -> Result<V, E>,
    ) -> Result<Arc<V>, String> {
        self.build_or_share(key, current, build, false)
    }

    /// [`Cache::get`] that builds even when the stored entry is fresh (a refresh asked for by hand); a build already
    /// running at this generation is shared. A failure keeps the stored entry.
    pub fn refresh<E: std::fmt::Display>(
        &self,
        key: &K,
        current: impl Fn() -> u64,
        build: impl Fn() -> Result<V, E>,
    ) -> Result<Arc<V>, String> {
        self.build_or_share(key, current, build, true)
    }

    fn build_or_share<E: std::fmt::Display>(
        &self,
        key: &K,
        current: impl Fn() -> u64,
        build: impl Fn() -> Result<V, E>,
        force: bool,
    ) -> Result<Arc<V>, String> {
        for attempt in 0..=REBUILDS {
            let generation = current();
            let flight = {
                let mut slots = lock(&self.slots);
                match slots.get(key) {
                    Some(Slot::Ready(entry))
                        if !(force && attempt == 0) && self.fresh(entry, generation) =>
                    {
                        self.hits.fetch_add(1, Ordering::Relaxed);
                        return Ok(Arc::clone(&entry.value));
                    }
                    Some(Slot::Building { flight, .. }) if flight.generation == generation => {
                        // Someone builds this generation already: wait for theirs.
                        Err(Arc::clone(flight))
                    }
                    _ => {
                        let last = match slots.remove(key) {
                            Some(Slot::Ready(entry)) => Some(entry),
                            Some(Slot::Building { last, .. }) => last,
                            None => None,
                        };
                        let flight = Arc::new(Flight {
                            generation,
                            done: Mutex::new(None),
                            ready: Condvar::new(),
                        });
                        slots.insert(
                            key.clone(),
                            Slot::Building {
                                flight: Arc::clone(&flight),
                                last,
                            },
                        );
                        Ok(flight)
                    }
                }
            };
            match flight {
                Err(theirs) => {
                    let mut done = lock(&theirs.done);
                    while done.is_none() {
                        done = theirs
                            .ready
                            .wait(done)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    return done.clone().expect("set before the waiters wake");
                }
                Ok(mine) => {
                    self.builds.fetch_add(1, Ordering::Relaxed);
                    let outcome = build().map(Arc::new).map_err(|e| e.to_string());
                    let still = current() == generation;
                    {
                        let mut slots = lock(&self.slots);
                        let last = match slots.remove(key) {
                            Some(Slot::Building { last, .. }) => last,
                            Some(Slot::Ready(entry)) => Some(entry),
                            None => None,
                        };
                        match (&outcome, still) {
                            (Ok(value), true) => {
                                slots.insert(
                                    key.clone(),
                                    Slot::Ready(Entry {
                                        value: Arc::clone(value),
                                        generation,
                                        built: Instant::now(),
                                    }),
                                );
                            }
                            // Failed, or the source moved while building: keep what was there before.
                            _ => {
                                if let Some(last) = last {
                                    slots.insert(key.clone(), Slot::Ready(last));
                                }
                            }
                        }
                    }
                    *lock(&mine.done) = Some(outcome.clone());
                    mine.ready.notify_all();
                    if still || attempt == REBUILDS || outcome.is_err() {
                        return outcome;
                    }
                }
            }
        }
        unreachable!("the last attempt returns")
    }

    /// Stale-while-revalidate: the last good value for `key` at once, even when stale, and a refresh in the background
    /// (on a thread of its own, single-flight) when it is stale or missing. `None` until a first refresh succeeds.
    pub fn get_or_refresh<E: std::fmt::Display + Send + 'static>(
        self: &Arc<Self>,
        key: &K,
        current: impl Fn() -> u64 + Send + 'static,
        build: impl Fn() -> Result<V, E> + Send + 'static,
    ) -> Option<Arc<V>> {
        let generation = current();
        let (value, start) = {
            let slots = lock(&self.slots);
            match slots.get(key) {
                Some(Slot::Ready(entry)) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    (
                        Some(Arc::clone(&entry.value)),
                        !self.fresh(entry, generation),
                    )
                }
                Some(Slot::Building { last, .. }) => {
                    (last.as_ref().map(|e| Arc::clone(&e.value)), false)
                }
                None => (None, true),
            }
        };
        if start {
            let cache = Arc::clone(self);
            let key = key.clone();
            std::thread::spawn(move || {
                let _ = cache.get(&key, current, build);
            });
        }
        value
    }

    /// The stored entry for `key`, fresh or not, without building.
    pub fn peek(&self, key: &K) -> Option<Entry<V>> {
        match lock(&self.slots).get(key) {
            Some(Slot::Ready(entry)) => Some(entry.clone()),
            Some(Slot::Building { last, .. }) => last.clone(),
            None => None,
        }
    }

    /// Puts a value built elsewhere (a background refresh, a restore from disk) at `generation`.
    pub fn put(&self, key: K, value: V, generation: u64) {
        let entry = Entry {
            value: Arc::new(value),
            generation,
            built: Instant::now(),
        };
        let mut slots = lock(&self.slots);
        match slots.get_mut(&key) {
            Some(Slot::Building { last, .. }) => *last = Some(entry),
            _ => {
                slots.insert(key, Slot::Ready(entry));
            }
        }
    }

    /// Drops the entry for `key` (a removed source).
    pub fn remove(&self, key: &K) {
        lock(&self.slots).remove(key);
    }

    /// How many answers came from a stored entry, and how many builds ran.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.builds.load(Ordering::Relaxed),
        )
    }
}

/// The ETag of a response body as sent: a weak validator over its bytes (`W/"<hex>"`).
pub fn etag(body: &[u8]) -> String {
    let hash = Sha256::digest(body);
    let hex: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
    format!("W/\"{hex}\"")
}

/// Whether a request's `If-None-Match` header names `tag` (or is `*`): the answer is then `304 Not Modified`.
pub fn not_modified(if_none_match: Option<&str>, tag: &str) -> bool {
    let Some(header) = if_none_match else {
        return false;
    };
    let bare = |t: &str| {
        t.trim()
            .trim_start_matches("W/")
            .trim_matches('"')
            .to_string()
    };
    let want = bare(tag);
    header
        .split(',')
        .any(|t| t.trim() == "*" || bare(t) == want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn a_fresh_entry_is_answered_without_a_build() {
        let cache: Cache<&str, u32> = Cache::new(Duration::from_secs(60));
        let builds = AtomicUsize::new(0);
        let build = || -> Result<u32, String> {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(7)
        };
        assert_eq!(*cache.get(&"a", || 1, build).unwrap(), 7);
        assert_eq!(*cache.get(&"a", || 1, build).unwrap(), 7);
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        // A new generation rebuilds.
        assert_eq!(*cache.get(&"a", || 2, build).unwrap(), 7);
        assert_eq!(builds.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_refresh_builds_even_when_fresh_and_keeps_the_last_on_failure() {
        let cache: Cache<&str, u32> = Cache::new(Duration::from_secs(60));
        cache
            .get(&"a", || 1, || -> Result<u32, String> { Ok(1) })
            .unwrap();
        assert_eq!(
            *cache
                .refresh(&"a", || 1, || -> Result<u32, String> { Ok(2) })
                .unwrap(),
            2
        );
        assert!(
            cache
                .refresh(&"a", || 1, || -> Result<u32, String> { Err("down".into()) })
                .is_err()
        );
        assert_eq!(*cache.peek(&"a").unwrap().value, 2);
    }

    #[test]
    fn concurrent_misses_share_one_build() {
        let cache: Arc<Cache<&'static str, u32>> = Arc::new(Cache::new(Duration::from_secs(60)));
        let builds = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let (cache, builds) = (Arc::clone(&cache), Arc::clone(&builds));
                std::thread::spawn(move || {
                    *cache
                        .get(
                            &"k",
                            || 1,
                            || -> Result<u32, String> {
                                builds.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_millis(50));
                                Ok(3)
                            },
                        )
                        .unwrap()
                })
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), 3);
        }
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_build_that_started_before_an_edit_never_overwrites_it() {
        // The source: a value and its generation, edited while the first build runs.
        let gens: Arc<Generations<&'static str>> = Arc::new(Generations::new());
        let source = Arc::new(Mutex::new(1u32));
        let cache: Arc<Cache<&'static str, u32>> = Arc::new(Cache::new(Duration::from_secs(60)));
        let started = Arc::new(std::sync::Barrier::new(2));
        let slow = {
            let (gens, source, cache, started) = (
                Arc::clone(&gens),
                Arc::clone(&source),
                Arc::clone(&cache),
                Arc::clone(&started),
            );
            std::thread::spawn(move || {
                let first = AtomicUsize::new(0);
                cache
                    .get(
                        &"p",
                        || gens.get(&"p"),
                        || -> Result<u32, String> {
                            let seen = *lock(&source);
                            if first.fetch_add(1, Ordering::SeqCst) == 0 {
                                started.wait();
                                std::thread::sleep(Duration::from_millis(50));
                            }
                            Ok(seen)
                        },
                    )
                    .unwrap()
            })
        };
        started.wait();
        {
            let mut value = lock(&source);
            *value = 2;
            gens.bump(&"p");
        }
        let answered = *slow.join().unwrap();
        assert_eq!(answered, 2, "the build saw the edit happen and built again");
        let stored = cache.peek(&"p").unwrap();
        assert_eq!(*stored.value, 2);
        assert_eq!(stored.generation, gens.get(&"p"));
    }

    #[test]
    fn stale_while_revalidate_answers_at_once_and_refreshes() {
        let cache: Arc<Cache<&'static str, u32>> = Arc::new(Cache::new(Duration::from_secs(60)));
        assert_eq!(
            cache.get_or_refresh(&"m", || 1, || -> Result<u32, String> { Ok(5) }),
            None
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while cache.peek(&"m").is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            cache
                .get_or_refresh(&"m", || 1, || -> Result<u32, String> { Ok(6) })
                .as_deref(),
            Some(&5)
        );
        // A new generation: the stale value now, the new one after the refresh.
        assert_eq!(
            cache
                .get_or_refresh(&"m", || 2, || -> Result<u32, String> { Ok(6) })
                .as_deref(),
            Some(&5)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while cache.peek(&"m").is_some_and(|e| *e.value == 5) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*cache.peek(&"m").unwrap().value, 6);
    }

    #[test]
    fn a_failed_build_keeps_the_last_good_value() {
        let cache: Cache<&str, u32> = Cache::new(Duration::from_secs(60));
        cache
            .get(&"x", || 1, || -> Result<u32, String> { Ok(1) })
            .unwrap();
        assert!(
            cache
                .get(&"x", || 2, || -> Result<u32, String> { Err("down".into()) })
                .is_err()
        );
        assert_eq!(*cache.peek(&"x").unwrap().value, 1);
    }

    #[test]
    fn etags_match_their_body_only() {
        let tag = etag(b"[1,2,3]");
        assert!(not_modified(Some(&tag), &tag));
        assert!(not_modified(Some(&format!("\"x\", {tag}")), &tag));
        assert!(not_modified(Some("*"), &tag));
        assert!(!not_modified(Some(&etag(b"[1,2]")), &tag));
        assert!(!not_modified(None, &tag));
    }
}
