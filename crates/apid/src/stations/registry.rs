//! The station registry: the workstations the owner enrolled, their facts, settings and state, and the
//! join codes that enroll them. Saved in Postgres as the `stations` component, as the nodes are. A
//! station's token is random and never stored: only its SHA-256 is.
//!
//! A join code is a join token (`apz_trust::token`, the same text a node's code is), used once, within
//! its expiry, and refused after [`JOIN_TRIES_PER_CODE`] failed tries. A machine that reports the hub's
//! own hostname, or whose hostname cannot be told apart from it, is refused before the code is used
//! (containment: a workstation is never the machine that runs apid).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use apz_station::{Facts, StationSettings};
use serde::{Deserialize, Serialize};

use super::leases::{LeaseView, Leases};
use super::link::StationLink;
use super::sync::SyncPause;
use super::vms::VmRecord;
use crate::nodes::{
    self, Code, CodeView, DEFAULT_TTL_MINUTES, JOIN_TRIES_PER_CODE, MAX_CODE_MINUTES,
};

/// A station is offline when its link has been silent this long.
pub const OFFLINE_AFTER_MS: u64 = 30_000;
/// The longest a station's name may be, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The one message every failed redemption gets, so a caller learns nothing about the code.
pub const INVALID_CODE: &str = "this join code is not valid";
/// The message a refused token gets.
pub const INVALID_TOKEN: &str = "this workstation token is not valid";

/// One enrolled workstation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Station {
    /// `w<seq>`.
    pub id: String,
    pub name: String,
    /// The daemon's version, from its last hello.
    #[serde(default)]
    pub version: String,
    /// The join code it came from (`j<seq>`).
    pub code: String,
    /// SHA-256 of its token, hex.
    pub token_sha256: String,
    pub joined_ms: u64,
    /// When its link last spoke (a hello, or the link's end).
    #[serde(default)]
    pub last_seen_ms: u64,
    /// The machine, as the daemon last reported it.
    pub facts: Facts,
    /// The hub-side settings, pushed to the daemon on every link and on every change.
    #[serde(default)]
    pub settings: StationSettings,
    #[serde(default)]
    pub draining: bool,
    #[serde(default)]
    pub revoked: bool,
    /// Where it joined from (for the owner's eyes only).
    #[serde(default)]
    pub address: String,
    /// The work-folder sync's pause, while it holds (sync.rs): why, and the deletions it holds back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_pause: Option<SyncPause>,
}

/// A station as the console lists it.
#[derive(Debug, Clone, Serialize)]
pub struct StationView {
    pub id: String,
    pub name: String,
    /// "online" | "offline" | "draining" | "revoked".
    pub status: &'static str,
    pub online: bool,
    pub draining: bool,
    pub revoked: bool,
    pub version: String,
    pub code: String,
    pub joined_ms: u64,
    pub last_seen_ms: u64,
    pub address: String,
    pub facts: Facts,
    pub settings: StationSettings,
    pub leases: Vec<LeaseView>,
}

/// Why a join was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Too many attempts from one address.
    RateLimited,
    /// The code is wrong, used, revoked, expired or out of tries; or the request is malformed.
    Invalid(String),
    /// The machine is the hub's own, or cannot be told apart from it.
    OwnHost(String),
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    stations: Vec<Station>,
    codes: Vec<Code>,
    /// The workstation VMs the hub makes on its nodes (vms.rs).
    #[serde(default)]
    vms: Vec<VmRecord>,
}

#[derive(Default)]
pub struct Store {
    inner: Mutex<Persisted>,
    /// The live link per station: the one the daemon currently holds open.
    links: Mutex<BTreeMap<String, Arc<StationLink>>>,
    join_attempts: Mutex<BTreeMap<String, Vec<u64>>>,
    /// The leases (in memory: a lease belongs to a live link, so a restart ends them all).
    pub leases: Leases,
    /// The live views: their viewers, streams and thumbnails (screen.rs).
    pub screen: super::screen::Live,
    /// The sessions the owner has taken, and the policies in force on them (takeover.rs).
    pub taken: super::takeover::Sessions,
    /// The recordings of the open leases (their index rows are in the database, see `recordings`).
    pub recordings: super::recordings::Recordings,
    /// The work-folder sync passes in progress (sync.rs).
    pub sync: super::sync::Gate,
    /// When each session's owner input was last audited (a takeover is one row per window, not one per key).
    takeovers: Mutex<BTreeMap<u64, u64>>,
    /// The last link generation handed out.
    link_seq: std::sync::atomic::AtomicU64,
    /// The stations whose VM is being snapshotted or reverted: their leases wait (vms.rs).
    resetting: Mutex<BTreeSet<String>>,
}

/// Whether a machine reporting `reported` may be enrolled or linked by a hub whose hostname is `hub`:
/// both names must be known, and differ on their first label (so `box.local` and `box` are the same).
pub fn may_join(reported: &str, hub: &str) -> bool {
    let label = |name: &str| {
        name.trim()
            .trim_end_matches('.')
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let (reported, hub) = (label(reported), label(hub));
    !reported.is_empty() && !hub.is_empty() && reported != hub
}

/// Whether a workstation's `facts` may join a hub named `hub` on the machine whose identifier is `hub_machine`:
/// [`may_join`] by name, and not the hub's own machine by identifier. A machine that reports no identifier is judged
/// by its name alone.
pub fn may_join_facts(facts: &Facts, hub: &str, hub_machine: Option<&str>) -> bool {
    may_join(&facts.hostname, hub) && !same_machine(facts.machine_id.as_deref(), hub_machine)
}

fn same_machine(reported: Option<&str>, hub: Option<&str>) -> bool {
    match (reported.map(str::trim), hub.map(str::trim)) {
        (Some(reported), Some(hub)) => !reported.is_empty() && reported.eq_ignore_ascii_case(hub),
        _ => false,
    }
}

/// A usable name: 1 to [`MAX_NAME_CHARS`] characters, none of them control characters.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(char::is_control)
}

fn code_id_prefix() -> char {
    'j'
}

fn station_id_prefix() -> char {
    'w'
}

impl Store {
    fn guard(&self) -> Result<std::sync::MutexGuard<'_, Persisted>, String> {
        self.inner.lock().map_err(|e| e.to_string())
    }

    pub fn snapshot(&self) -> String {
        self.inner
            .lock()
            .ok()
            .and_then(|p| serde_json::to_string(&*p).ok())
            .unwrap_or_default()
    }

    pub fn restore(&self, json: &str) -> Result<(), String> {
        let p: Persisted = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.guard()? = p;
        Ok(())
    }

    pub fn list(&self) -> Vec<Station> {
        self.inner
            .lock()
            .map(|p| p.stations.clone())
            .unwrap_or_default()
    }

    pub fn station(&self, id: &str) -> Option<Station> {
        self.list().into_iter().find(|s| s.id == id)
    }

    /// The workstation VMs the hub knows.
    pub fn vms(&self) -> Vec<VmRecord> {
        self.inner.lock().map(|p| p.vms.clone()).unwrap_or_default()
    }

    pub fn vm(&self, id: &str) -> Option<VmRecord> {
        self.vms().into_iter().find(|v| v.id == id)
    }

    /// The VM whose station is `station`, when it enrolled from one.
    pub fn vm_of_station(&self, station: &str) -> Option<VmRecord> {
        self.vms()
            .into_iter()
            .find(|v| v.station.as_deref() == Some(station))
    }

    /// The VM a join code was issued for (`j<seq>`).
    pub fn vm_of_code(&self, code: &str) -> Option<VmRecord> {
        self.vms().into_iter().find(|v| v.code == code)
    }

    /// The next VM id (`v<seq>`).
    pub fn new_vm_id(&self) -> Result<String, String> {
        let p = self.guard()?;
        Ok(format!(
            "v{}",
            nodes::next_seq(p.vms.iter().map(|v| v.id.as_str()), 'v')
        ))
    }

    pub fn add_vm(&self, record: VmRecord) -> Result<(), String> {
        self.guard()?.vms.push(record);
        Ok(())
    }

    /// Records that the station enrolled with the VM whose join code is `code`.
    pub fn link_vm(&self, code: &str, station: &str) {
        let Ok(mut p) = self.guard() else { return };
        if let Some(vm) = p.vms.iter_mut().find(|v| v.code == code) {
            vm.station = Some(station.to_string());
        }
    }

    pub fn remove_vm(&self, id: &str) -> Option<VmRecord> {
        let mut p = self.guard().ok()?;
        let at = p.vms.iter().position(|v| v.id == id)?;
        Some(p.vms.remove(at))
    }

    /// Marks a station as resetting. False when it already is, or the marks cannot be read.
    pub fn begin_reset(&self, station: &str) -> bool {
        self.resetting
            .lock()
            .map(|mut set| set.insert(station.to_string()))
            .unwrap_or(false)
    }

    pub fn end_reset(&self, station: &str) {
        if let Ok(mut set) = self.resetting.lock() {
            set.remove(station);
        }
    }

    pub fn is_resetting(&self, station: &str) -> bool {
        self.resetting
            .lock()
            .map(|set| set.contains(station))
            .unwrap_or(false)
    }

    /// Issues a join code for `hub_url` pinned to the hub's certificate: its id, its text (shown once) and
    /// its expiry in ms.
    pub fn issue_code(
        &self,
        hub_url: &str,
        cert: [u8; 32],
        label: &str,
        ttl_minutes: u64,
        now: u64,
    ) -> Result<(String, String, u64), String> {
        let ttl = match ttl_minutes {
            0 => DEFAULT_TTL_MINUTES,
            m => m.min(MAX_CODE_MINUTES),
        };
        let token = apz_trust::token::JoinToken::issue(hub_url, cert, now / 1000, ttl * 60);
        let text = token.encode();
        let expires_ms = now + ttl * 60_000;
        let mut p = self.guard()?;
        let id = format!(
            "{}{}",
            code_id_prefix(),
            nodes::next_seq(p.codes.iter().map(|c| c.id.as_str()), code_id_prefix())
        );
        p.codes.push(Code {
            id: id.clone(),
            label: label.trim().chars().take(64).collect(),
            secret_sha256: nodes::hex(&token.secret_hash()),
            created_ms: now,
            expires_ms,
            used_by: None,
            revoked: false,
            failures: 0,
        });
        Ok((id, text, expires_ms))
    }

    pub fn codes(&self, now: u64) -> Vec<CodeView> {
        self.inner
            .lock()
            .map(|p| {
                p.codes
                    .iter()
                    .map(|c| CodeView {
                        id: c.id.clone(),
                        label: c.label.clone(),
                        created_ms: c.created_ms,
                        expires_ms: c.expires_ms,
                        state: nodes::code_state(c, now).to_string(),
                        used_by: c.used_by.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Revokes a code no station has used yet.
    pub fn revoke_code(&self, id: &str) -> Result<(), String> {
        let mut p = self.guard()?;
        let code = p
            .codes
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("no such code")?;
        if code.used_by.is_some() {
            return Err("that code was already used".into());
        }
        code.revoked = true;
        Ok(())
    }

    /// Redeems a join code for a new station: the station, and its token (shown once).
    pub fn redeem(
        &self,
        source: &str,
        code_text: &str,
        facts: Facts,
        name: Option<&str>,
        hub_host: &str,
        now: u64,
    ) -> Result<(Station, String), Refusal> {
        nodes::admit_join(&self.join_attempts, source, now).map_err(|_| Refusal::RateLimited)?;
        let token = apz_trust::token::JoinToken::decode(code_text)
            .map_err(|_| Refusal::Invalid(INVALID_CODE.into()))?;
        let hash = nodes::hex(&token.secret_hash());
        let mut p = self.guard().map_err(Refusal::Invalid)?;
        let Persisted {
            stations, codes, ..
        } = &mut *p;
        let mut found = None;
        for (i, code) in codes.iter().enumerate() {
            if apz_trust::ct_eq(code.secret_sha256.as_bytes(), hash.as_bytes()) {
                found = Some(i);
            }
        }
        let i = found.ok_or_else(|| Refusal::Invalid(INVALID_CODE.into()))?;
        let code = &mut codes[i];
        let usable = code.used_by.is_none()
            && !code.revoked
            && now < code.expires_ms
            && code.failures < JOIN_TRIES_PER_CODE;
        if !usable {
            code.failures = code.failures.saturating_add(1);
            return Err(Refusal::Invalid(INVALID_CODE.into()));
        }
        // Containment, before the code is spent: the hub's own machine is never a workstation.
        if !may_join_facts(&facts, hub_host, apz_os::machine::id().as_deref()) {
            return Err(Refusal::OwnHost(
                "this machine is the hub's own host, or its hostname cannot be told apart from the hub's: a workstation is another machine".into(),
            ));
        }
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(facts.hostname.trim())
            .to_string();
        if !valid_name(&name) {
            code.failures = code.failures.saturating_add(1);
            return Err(Refusal::Invalid(
                "the name must be 1 to 64 printable characters".into(),
            ));
        }

        let seq = nodes::next_seq(stations.iter().map(|s| s.id.as_str()), station_id_prefix());
        let token_text = apz_trust::b64(&apz_trust::random::<32>());
        let station = Station {
            id: format!("{}{seq}", station_id_prefix()),
            name,
            version: String::new(),
            code: code.id.clone(),
            token_sha256: nodes::hex(&apz_trust::sha256(token_text.as_bytes())),
            joined_ms: now,
            last_seen_ms: now,
            facts,
            settings: StationSettings::default(),
            draining: false,
            revoked: false,
            address: source.to_string(),
            sync_pause: None,
        };
        code.used_by = Some(station.id.clone());
        stations.push(station.clone());
        Ok((station, token_text))
    }

    /// Removes a station that was just enrolled whose record could not be written (the join is undone).
    pub fn undo_join(&self, id: &str) {
        if let Ok(mut p) = self.guard() {
            p.stations.retain(|s| s.id != id);
        }
    }

    /// The station a bearer token names. A revoked station's token is refused.
    pub fn authenticate(&self, bearer: &str) -> Result<Station, String> {
        let hash = nodes::hex(&apz_trust::sha256(bearer.as_bytes()));
        let p = self.guard()?;
        let station = p
            .stations
            .iter()
            .find(|s| apz_trust::ct_eq(s.token_sha256.as_bytes(), hash.as_bytes()))
            .ok_or_else(|| INVALID_TOKEN.to_string())?;
        if station.revoked {
            return Err("this workstation was revoked".into());
        }
        Ok(station.clone())
    }

    /// Records a daemon's hello: its version and facts, and that it spoke now.
    pub fn hello(&self, id: &str, version: &str, facts: Facts, now: u64) -> Result<(), String> {
        let mut p = self.guard()?;
        let station = p
            .stations
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or("no such workstation")?;
        station.version = version.to_string();
        station.facts = facts;
        station.last_seen_ms = now;
        Ok(())
    }

    pub fn set_facts(&self, id: &str, facts: Facts) {
        let Ok(mut p) = self.guard() else { return };
        if let Some(station) = p.stations.iter_mut().find(|s| s.id == id) {
            station.facts = facts;
        }
    }

    /// Records when a link last spoke, as its end leaves it.
    pub fn seen(&self, id: &str, now: u64) {
        let Ok(mut p) = self.guard() else { return };
        if let Some(station) = p.stations.iter_mut().find(|s| s.id == id) {
            station.last_seen_ms = now;
        }
    }

    pub fn revoke(&self, id: &str) -> Result<Station, String> {
        self.update(id, |s| s.revoked = true)
    }

    pub fn set_draining(&self, id: &str, on: bool) -> Result<Station, String> {
        self.update(id, |s| s.draining = on)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<Station, String> {
        if !valid_name(name) {
            return Err("the name must be 1 to 64 printable characters".into());
        }
        let name = name.trim().to_string();
        self.update(id, |s| s.name = name.clone())
    }

    pub fn set_settings(&self, id: &str, settings: StationSettings) -> Result<Station, String> {
        self.update(id, |s| s.settings = settings.clone())
    }

    /// The work-folder sync's pause on a station, while it holds.
    pub fn sync_pause(&self, id: &str) -> Option<SyncPause> {
        self.station(id).and_then(|s| s.sync_pause)
    }

    /// Sets or clears a station's work-folder sync pause. A station that is gone has none to set.
    pub fn set_sync_pause(&self, id: &str, pause: Option<SyncPause>) {
        let _ = self.update(id, |s| s.sync_pause = pause);
    }

    fn update(&self, id: &str, change: impl FnOnce(&mut Station)) -> Result<Station, String> {
        let mut p = self.guard()?;
        let station = p
            .stations
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or("no such workstation")?;
        change(station);
        Ok(station.clone())
    }

    /// Whether a takeover on `session` is due an audit row: at most one per `window` ms per session.
    pub fn takeover_due(&self, session: u64, now: u64, window: u64) -> bool {
        let Ok(mut taken) = self.takeovers.lock() else {
            return true;
        };
        match taken.get(&session) {
            Some(last) if now.saturating_sub(*last) < window => false,
            _ => {
                taken.insert(session, now);
                true
            }
        }
    }

    /// The next link generation (unique per hub run).
    pub fn next_link_id(&self) -> u64 {
        self.link_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// Attaches `link` as its station's link. The link it replaces is returned, for the caller to end.
    pub fn attach(&self, link: Arc<StationLink>) -> Option<Arc<StationLink>> {
        self.links
            .lock()
            .ok()?
            .insert(link.station().to_string(), link)
    }

    /// Detaches a station's link, when it is still the one `id` names (a replaced link leaves nothing behind).
    pub fn detach(&self, station: &str, id: u64) {
        let Ok(mut links) = self.links.lock() else {
            return;
        };
        if links.get(station).is_some_and(|l| l.id() == id) {
            links.remove(station);
        }
    }

    /// The station's live link, if the daemon holds one open.
    pub fn live(&self, station: &str) -> Option<Arc<StationLink>> {
        self.links
            .lock()
            .ok()?
            .get(station)
            .filter(|l| !l.is_closed())
            .cloned()
    }

    /// Every live link.
    pub fn live_links(&self) -> Vec<Arc<StationLink>> {
        self.links
            .lock()
            .map(|links| links.values().filter(|l| !l.is_closed()).cloned().collect())
            .unwrap_or_default()
    }

    /// Whether a station is online now: its link is open and spoke within [`OFFLINE_AFTER_MS`].
    pub fn is_online(&self, station: &str, now: u64) -> bool {
        self.live(station)
            .is_some_and(|link| now.saturating_sub(link.last_seen_ms()) < OFFLINE_AFTER_MS)
    }

    /// A station as the console lists it, with its leases.
    pub fn view(&self, station: Station, now: u64) -> StationView {
        let online = self.is_online(&station.id, now);
        let status = if station.revoked {
            "revoked"
        } else if station.draining {
            "draining"
        } else if online {
            "online"
        } else {
            "offline"
        };
        let last_seen_ms = self
            .live(&station.id)
            .map_or(station.last_seen_ms, |link| link.last_seen_ms());
        StationView {
            leases: self.leases.views(&station.id),
            id: station.id,
            name: station.name,
            status,
            online,
            draining: station.draining,
            revoked: station.revoked,
            version: station.version,
            code: station.code,
            joined_ms: station.joined_ms,
            last_seen_ms,
            address: station.address,
            facts: station.facts,
            settings: station.settings,
        }
    }

    /// Every station's view, in the order they joined.
    pub fn views(&self, now: u64) -> Vec<StationView> {
        self.list().into_iter().map(|s| self.view(s, now)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::{AccountMode, Form, Os};

    const NOW: u64 = 1_000_000_000;
    const HUB: &str = "hub-box";

    fn facts(hostname: &str) -> Facts {
        Facts {
            os: Os::Linux,
            os_version: "6.8".into(),
            arch: "x86_64".into(),
            hostname: hostname.into(),
            form: Form::Desktop,
            cpus: 8,
            ram_mb: 16384,
            gpus: vec![],
            displays: vec![],
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
            apps: 3,
            virtual_displays: true,
            ui_tree: true,
            machine_id: None,
        }
    }

    /// A code for `store`, returned with its join text.
    fn code(store: &Store, label: &str, ttl: u64, now: u64) -> (String, String) {
        let (id, text, _) = store
            .issue_code("https://hub.test:7700", [3; 32], label, ttl, now)
            .unwrap();
        (id, text)
    }

    #[test]
    fn a_code_joins_once_and_its_token_is_stored_only_as_a_hash() {
        let store = Store::default();
        let (code_id, text) = code(&store, "desk", 0, NOW);
        let (station, token) = store
            .redeem("203.0.113.7", &text, facts("desk-1"), None, HUB, NOW)
            .unwrap();
        assert_eq!(station.id, "w1");
        assert_eq!(
            station.name, "desk-1",
            "the name defaults to the reported hostname"
        );
        assert_eq!(station.code, code_id);
        assert_eq!(
            station.token_sha256,
            nodes::hex(&apz_trust::sha256(token.as_bytes()))
        );
        assert!(
            !store.snapshot().contains(&token),
            "the token itself is never saved"
        );
        assert_eq!(store.authenticate(&token).unwrap().id, "w1");
        assert!(store.authenticate("another").is_err());
        assert_eq!(store.codes(NOW)[0].state, "used");
        assert!(
            store
                .redeem("203.0.113.8", &text, facts("desk-2"), None, HUB, NOW)
                .is_err(),
            "a code joins once"
        );
    }

    #[test]
    fn an_expired_code_is_refused_and_a_revoked_one_too() {
        let store = Store::default();
        let (_, text) = code(&store, "old", 1, NOW);
        let later = NOW + 61_000;
        assert_eq!(store.codes(later)[0].state, "expired");
        assert_eq!(
            store
                .redeem("a", &text, facts("box"), None, HUB, later)
                .unwrap_err(),
            Refusal::Invalid(INVALID_CODE.into())
        );
        let (id, text2) = code(&store, "mine", 10, NOW);
        store.revoke_code(&id).unwrap();
        assert_eq!(store.codes(NOW)[1].state, "revoked");
        assert!(
            store
                .redeem("a", &text2, facts("box"), None, HUB, NOW)
                .is_err()
        );
        assert!(store.revoke_code("j9").is_err());
    }

    #[test]
    fn the_hubs_own_host_is_refused_and_the_code_is_kept_for_another_machine() {
        let store = Store::default();
        let (_, text) = code(&store, "desk", 10, NOW);
        assert!(matches!(
            store.redeem("127.0.0.1", &text, facts(HUB), None, HUB, NOW),
            Err(Refusal::OwnHost(_))
        ));
        assert!(matches!(
            store.redeem("192.168.1.9", &text, facts("HUB-BOX.local"), None, HUB, NOW),
            Err(Refusal::OwnHost(_))
        ));
        assert!(
            matches!(
                store.redeem("192.168.1.9", &text, facts(""), None, HUB, NOW),
                Err(Refusal::OwnHost(_))
            ),
            "an unnamed machine is not trusted"
        );
        assert!(
            matches!(
                store.redeem("192.168.1.9", &text, facts("desk"), None, "", NOW),
                Err(Refusal::OwnHost(_))
            ),
            "an unknown hub host refuses"
        );
        let (station, _) = store
            .redeem("192.168.1.9", &text, facts("desk"), None, HUB, NOW)
            .unwrap();
        assert_eq!(
            station.id, "w1",
            "the refused attempts did not use the code"
        );
    }

    #[test]
    fn a_wrong_code_burns_after_its_tries_and_a_bad_name_counts_as_one() {
        let store = Store::default();
        let (_, text) = code(&store, "desk", 10, NOW);
        assert!(
            store
                .redeem("a", &text, facts("desk"), Some(""), HUB, NOW)
                .is_ok(),
            "an empty name falls back to the hostname"
        );
        let (_, second) = code(&store, "two", 10, NOW);
        for _ in 0..JOIN_TRIES_PER_CODE {
            let bad = store
                .redeem("b", &second, facts("box"), Some("\u{7}bell"), HUB, NOW)
                .unwrap_err();
            assert!(
                matches!(bad, Refusal::Invalid(ref m) if m.contains("name")),
                "the matched code reports the name"
            );
        }
        assert_eq!(
            store
                .redeem("b", &second, facts("box"), None, HUB, NOW)
                .unwrap_err(),
            Refusal::Invalid(INVALID_CODE.into())
        );
    }

    #[test]
    fn joins_are_rate_limited_per_source() {
        let store = Store::default();
        for _ in 0..nodes::JOIN_RATE_PER_MINUTE {
            let _ = store.redeem("a", "apzj1.nope", facts("box"), None, HUB, NOW);
        }
        assert_eq!(
            store
                .redeem("a", "apzj1.nope", facts("box"), None, HUB, NOW)
                .unwrap_err(),
            Refusal::RateLimited
        );
    }

    #[test]
    fn a_name_is_one_to_sixty_four_printable_characters() {
        assert!(valid_name("desk"));
        assert!(!valid_name("   "));
        assert!(!valid_name("a\nb"));
        assert!(valid_name(&"x".repeat(MAX_NAME_CHARS)));
        assert!(!valid_name(&"x".repeat(MAX_NAME_CHARS + 1)));
    }

    #[test]
    fn may_join_compares_the_first_label_and_needs_both_names() {
        assert!(may_join("desk", "hub"));
        assert!(!may_join("hub.local", "HUB"));
        assert!(!may_join("", "hub"));
        assert!(!may_join("desk", ""));
    }

    #[test]
    fn the_hubs_own_machine_may_not_join_under_another_name() {
        let with_id = |host: &str, id: Option<&str>| Facts {
            machine_id: id.map(str::to_string),
            ..facts(host)
        };
        assert!(may_join_facts(
            &with_id("desk", Some("abc")),
            "hub",
            Some("def")
        ));
        assert!(
            !may_join_facts(&with_id("desk", Some("ABC")), "hub", Some("abc")),
            "the same machine, renamed, is still the hub's"
        );
        assert!(
            may_join_facts(&with_id("desk", None), "hub", Some("abc")),
            "no identifier: judged by name"
        );
        assert!(
            !may_join_facts(&with_id("hub", Some("def")), "hub", Some("abc")),
            "the name check still applies"
        );
        assert!(may_join_facts(&with_id("desk", Some("abc")), "hub", None));
    }

    #[test]
    fn revoke_drain_rename_and_settings_change_the_record() {
        let store = Store::default();
        let (_, text) = code(&store, "desk", 10, NOW);
        let (station, token) = store
            .redeem("a", &text, facts("desk"), None, HUB, NOW)
            .unwrap();
        assert!(store.set_draining(&station.id, true).unwrap().draining);
        assert!(store.rename(&station.id, "  \t ").is_err());
        assert_eq!(
            store.rename(&station.id, " studio ").unwrap().name,
            "studio"
        );
        let settings = StationSettings {
            record: false,
            ..StationSettings::default()
        };
        assert_eq!(
            store
                .set_settings(&station.id, settings.clone())
                .unwrap()
                .settings,
            settings
        );
        let view = store.view(store.station("w1").unwrap(), NOW);
        assert_eq!(view.status, "draining");
        assert_eq!(view.code, station.code);
        store.revoke("w1").unwrap();
        assert_eq!(
            store.view(store.station("w1").unwrap(), NOW).status,
            "revoked"
        );
        assert_eq!(
            store.authenticate(&token).unwrap_err(),
            "this workstation was revoked"
        );
        assert!(store.revoke("w9").is_err());
    }

    #[test]
    fn a_new_link_replaces_the_old_and_the_old_one_leaving_does_not_remove_the_new() {
        let store = Store::default();
        let (first, _first_rx) = StationLink::new("w1", store.next_link_id(), NOW);
        assert!(store.attach(Arc::clone(&first)).is_none());
        let (second, _second_rx) = StationLink::new("w1", store.next_link_id(), NOW);
        let replaced = store
            .attach(Arc::clone(&second))
            .expect("the link it replaces is returned, to be ended");
        assert_eq!(replaced.id(), first.id());
        store.detach("w1", first.id());
        assert_eq!(
            store.live("w1").map(|l| l.id()),
            Some(second.id()),
            "the replacement stays attached"
        );
        store.detach("w1", second.id());
        assert!(store.live("w1").is_none());
    }

    #[test]
    fn a_station_silent_for_thirty_seconds_is_offline_and_a_frame_brings_it_back() {
        let store = Store::default();
        let (link, _rx) = StationLink::new("w1", store.next_link_id(), NOW);
        store.attach(Arc::clone(&link));
        assert!(store.is_online("w1", NOW + OFFLINE_AFTER_MS - 1));
        assert!(
            !store.is_online("w1", NOW + OFFLINE_AFTER_MS),
            "silent for 30 s: offline"
        );
        link.touch(NOW + OFFLINE_AFTER_MS);
        assert!(store.is_online("w1", NOW + OFFLINE_AFTER_MS + 1_000));
        link.finish();
        assert!(
            !store.is_online("w1", NOW + OFFLINE_AFTER_MS + 1_000),
            "a finished link is not online"
        );
        assert!(
            store.live_links().is_empty(),
            "and a STOP ALL reaches no finished link"
        );
    }

    #[test]
    fn a_takeover_is_audited_once_per_window_per_session() {
        let store = Store::default();
        assert!(store.takeover_due(9, NOW, 30_000));
        assert!(!store.takeover_due(9, NOW + 29_000, 30_000));
        assert!(store.takeover_due(9, NOW + 30_000, 30_000));
        assert!(
            store.takeover_due(10, NOW, 30_000),
            "another session has its own window"
        );
    }

    #[test]
    fn the_registry_survives_a_snapshot_and_restore() {
        let store = Store::default();
        let (_, text) = code(&store, "desk", 10, NOW);
        let (_, token) = store
            .redeem("a", &text, facts("desk"), Some("front"), HUB, NOW)
            .unwrap();
        let json = store.snapshot();
        let restored = Store::default();
        restored.restore(&json).unwrap();
        assert_eq!(restored.station("w1").unwrap().name, "front");
        assert_eq!(restored.authenticate(&token).unwrap().id, "w1");
        assert!(restored.restore("not json").is_err());
        assert_eq!(
            restored.list().len(),
            1,
            "a refused restore changes nothing"
        );
    }
}
