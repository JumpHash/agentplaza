//! Share links (docs/design/sharing.md): "anyone with the link" for an artifact or a Library file, served by this
//! hub at `{hub}/artifact/{id}` and, through the agentplaza.io relay, at `https://agentplaza.io/artifact/{id}`.
//!
//! The store keeps the shares in `<state_dir>/shares.json` (rewritten atomically on every change), the events in
//! `<state_dir>/share_events.jsonl` (appended), and the HMAC key for unlock tokens in `<state_dir>/share_secret`.
//! Migration 0015 creates the `shares` and `share_events` tables for the Postgres-backed store that the hub's DB
//! snapshot mirrors into. This release the file store is the source of truth, and nothing writes those tables yet.

pub mod public;
pub mod relay;
pub mod request;
pub mod routes;

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// The events kept in memory for `Store::events`. Older events stay in the file and count toward `counts`.
const EVENTS_IN_MEMORY: usize = 10_000;

/// The events file is rewritten with its last `EVENTS_IN_MEMORY` events once it holds more than this many lines.
const EVENTS_ON_DISK_MAX: usize = 20_000;

/// The failed unlocks of one share allowed in `UNLOCK_WINDOW_MS`. The next attempt is refused without running argon2.
pub const UNLOCK_LIMIT: usize = 10;

/// The window in which failed unlocks of one share are counted: 10 minutes, in milliseconds.
pub const UNLOCK_WINDOW_MS: u64 = 600_000;

/// How long an unlock token lasts: 7 days, in milliseconds.
pub const UNLOCK_TTL_MS: u64 = 604_800_000;

/// What a share points at. `kind` is "artifact" or "file"; `id` is the artifact's id or the Library file's id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub kind: String,
    pub id: String,
}

/// One share link.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Share {
    /// 32 lowercase hex characters: 128 random bits, the token in the link.
    pub id: String,
    pub target: Target,
    /// The artifact version the link serves. `None` serves the latest.
    pub version: Option<u32>,
    /// An argon2 PHC string when the link needs a password. The password itself is never stored.
    pub password_hash: Option<String>,
    /// Unix milliseconds after which the link stops working.
    pub expires_ms: Option<u64>,
    pub allow_download: bool,
    pub created_by: String,
    pub created_ms: u64,
    /// Unix milliseconds when the link was revoked. A revoked link never works again.
    pub revoked_ms: Option<u64>,
    pub title: String,
}

/// One recorded use of a share.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShareEvent {
    pub share: String,
    pub at_ms: u64,
    /// "view", "download" or "unlock_failed".
    pub kind: String,
    /// "network" (this hub) or "relay" (agentplaza.io).
    pub via: String,
}

/// The options a new share is created with.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub version: Option<u32>,
    /// A plain password to hash. An empty string means no password.
    pub password: Option<String>,
    pub expires_ms: Option<u64>,
    pub allow_download: bool,
}

/// What a request for a share gets back.
pub enum Access {
    /// The share works for this request.
    Ok(Share),
    /// No share has this id.
    Missing,
    /// The share was revoked.
    Revoked,
    /// The share's expiry has passed.
    Expired,
    /// The share is live but needs its password, or an unlock token, first.
    NeedsPassword,
}

/// The share store. The shares, the events and the counts sit behind one lock, and the files are written under it.
pub struct Store {
    shares_path: PathBuf,
    events_path: PathBuf,
    secret: [u8; 32],
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Every share in creation order, so the newest is last.
    shares: Vec<Share>,
    /// The last `EVENTS_IN_MEMORY` events, oldest first.
    events: VecDeque<ShareEvent>,
    /// Views and downloads per share, counted over the whole events file.
    counts: HashMap<String, (u64, u64)>,
    /// The lines the events file holds, so the file is capped without counting it on every write.
    events_on_disk: usize,
    /// The times of the recent failed unlocks per share, oldest first. Only the last `UNLOCK_WINDOW_MS` are kept.
    failures: HashMap<String, VecDeque<u64>>,
}

impl Store {
    /// Loads the shares and the events from `state_dir`, and the unlock secret (created on first load). A shares
    /// file that does not parse is moved aside as `shares.json.corrupt-<unix ms>` and logged, and the store starts
    /// empty.
    pub fn load(state_dir: &Path) -> Store {
        let shares_path = state_dir.join("shares.json");
        let events_path = state_dir.join("share_events.jsonl");
        let shares = read_shares(&shares_path);
        let (events, counts, events_on_disk) = read_events(&events_path);
        Store {
            secret: load_secret(&state_dir.join("share_secret")),
            shares_path,
            events_path,
            inner: Mutex::new(Inner {
                shares,
                events,
                counts,
                events_on_disk,
                failures: HashMap::new(),
            }),
        }
    }

    /// Creates a share of `target`, saves it and returns it. The id is 128 random bits. A password is kept as an
    /// argon2 hash. A failed save is named on stderr, and the share stays in memory until the next save.
    pub fn create(&self, target: Target, title: String, opts: Options, created_by: &str) -> Share {
        let mut bytes = [0u8; 16];
        apz_os::random::fill(&mut bytes).expect("share ids need OS randomness");
        let share = Share {
            id: hex(&bytes),
            target,
            version: opts.version,
            password_hash: opts
                .password
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(hash_password),
            expires_ms: opts.expires_ms,
            allow_download: opts.allow_download,
            created_by: created_by.to_string(),
            created_ms: unix_ms(),
            revoked_ms: None,
            title,
        };
        let mut inner = self.lock();
        inner.shares.push(share.clone());
        self.save(&inner);
        share
    }

    /// Changes the given fields of share `id` and saves it. The outer `Option` of each argument says whether the
    /// field changes; `Some(None)` clears it. An empty password clears it. Returns `None` for an unknown id.
    pub fn change(
        &self,
        id: &str,
        password: Option<Option<String>>,
        expires_ms: Option<Option<u64>>,
        allow_download: Option<bool>,
        version: Option<Option<u32>>,
    ) -> Option<Share> {
        let mut inner = self.lock();
        let share = inner.shares.iter_mut().find(|s| s.id == id)?;
        if let Some(password) = password {
            share.password_hash = password
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(hash_password);
        }
        if let Some(expires) = expires_ms {
            share.expires_ms = expires;
        }
        if let Some(allow) = allow_download {
            share.allow_download = allow;
        }
        if let Some(version) = version {
            share.version = version;
        }
        let changed = share.clone();
        self.save(&inner);
        Some(changed)
    }

    /// Revokes share `id` (the first revocation time is kept) and saves it. The revocation holds in memory even when
    /// the save fails, and the error is returned so the owner can be told to try again; a retry saves it again.
    /// `Ok(None)` is an unknown id.
    pub fn revoke(&self, id: &str) -> std::io::Result<Option<Share>> {
        let mut inner = self.lock();
        let Some(share) = inner.shares.iter_mut().find(|s| s.id == id) else {
            return Ok(None);
        };
        if share.revoked_ms.is_none() {
            share.revoked_ms = Some(unix_ms());
        }
        let revoked = share.clone();
        if let Err(e) = write_atomic(&self.shares_path, &shares_bytes(&inner.shares)) {
            eprintln!(
                "shares: save {} failed, revocation not on disk: {e}",
                self.shares_path.display()
            );
            return Err(e);
        }
        Ok(Some(revoked))
    }

    /// The share with `id`, whatever its state.
    pub fn get(&self, id: &str) -> Option<Share> {
        self.lock().shares.iter().find(|s| s.id == id).cloned()
    }

    /// Every share, or the shares of `target` only, newest first.
    pub fn list(&self, target: Option<&Target>) -> Vec<Share> {
        self.lock()
            .shares
            .iter()
            .rev()
            .filter(|s| target.is_none_or(|t| s.target == *t))
            .cloned()
            .collect()
    }

    /// The shares that are neither revoked nor expired now, newest first.
    pub fn live(&self) -> Vec<Share> {
        let now = unix_ms();
        self.lock()
            .shares
            .iter()
            .rev()
            .filter(|s| s.revoked_ms.is_none() && !expired(s, now))
            .cloned()
            .collect()
    }

    /// Whether a request at `now_ms` may use share `id`. A password-protected share is only `Ok` when `unlocked`
    /// (the caller checked a password or an unlock token). Revocation is checked before expiry.
    pub fn access(&self, id: &str, now_ms: u64, unlocked: bool) -> Access {
        let Some(share) = self.get(id) else {
            return Access::Missing;
        };
        if share.revoked_ms.is_some() {
            return Access::Revoked;
        }
        if expired(&share, now_ms) {
            return Access::Expired;
        }
        if share.password_hash.is_some() && !unlocked {
            return Access::NeedsPassword;
        }
        Access::Ok(share)
    }

    /// Whether `password` is the share's password. False when the share has none.
    pub fn check_password(&self, id: &str, password: &str) -> bool {
        match self.get(id).and_then(|s| s.password_hash) {
            Some(stored) => verify_password(&stored, password),
            None => false,
        }
    }

    /// The unlock token for share `id` at `now_ms`, or `None` when the share is missing or has no password. The token
    /// is `{expiry in hex}.{HMAC-SHA256 of the id, the password hash and the expiry, under the store's secret}`, so
    /// it lasts `UNLOCK_TTL_MS` and stops working when the password changes or is removed.
    pub fn unlock_token(&self, id: &str, now_ms: u64) -> Option<String> {
        let stored = self.get(id)?.password_hash?;
        let expiry = format!("{:x}", now_ms.saturating_add(UNLOCK_TTL_MS));
        Some(format!(
            "{expiry}.{}",
            hex(&self.mac(&unlock_message(id, &stored, &expiry)))
        ))
    }

    /// Whether `token` is a live unlock token of share `id` at `now_ms`: its expiry has not passed, and its MAC is
    /// the one the share's current password hash gives. The comparison takes the same time whatever the token.
    pub fn token_ok(&self, id: &str, token: &str, now_ms: u64) -> bool {
        let Some((expiry, mac)) = token.split_once('.') else {
            return false;
        };
        let Ok(expires_ms) = u64::from_str_radix(expiry, 16) else {
            return false;
        };
        let Some(stored) = self.get(id).and_then(|s| s.password_hash) else {
            return false;
        };
        let expected = hex(&self.mac(&unlock_message(id, &stored, expiry)));
        let fresh = now_ms < expires_ms;
        constant_time_eq(expected.as_bytes(), mac.as_bytes()) & fresh
    }

    /// Whether share `id` has had `UNLOCK_LIMIT` failed unlocks within the last `UNLOCK_WINDOW_MS` at `now_ms`. A
    /// blocked attempt must not run the password check.
    pub fn unlock_blocked(&self, id: &str, now_ms: u64) -> bool {
        let mut inner = self.lock();
        let Some(window) = inner.failures.get_mut(id) else {
            return false;
        };
        prune_failures(window, now_ms);
        let blocked = window.len() >= UNLOCK_LIMIT;
        if window.is_empty() {
            inner.failures.remove(id);
        }
        blocked
    }

    /// Records a use of share `id`: appended to the events file, kept in memory, and counted when it is a view
    /// or a download. A failed unlock also joins the share's failure window. Once the events file holds more than
    /// `EVENTS_ON_DISK_MAX` lines it is rewritten with its last `EVENTS_IN_MEMORY` events.
    pub fn record(&self, share: &str, kind: &str, via: &str) {
        let now = unix_ms();
        let event = ShareEvent {
            share: share.to_string(),
            at_ms: now,
            kind: kind.to_string(),
            via: via.to_string(),
        };
        let line = serde_json::to_string(&event).expect("a share event serializes");
        let mut inner = self.lock();
        if let Err(e) = append_line(&self.events_path, &line) {
            eprintln!("shares: record {}: {e}", self.events_path.display());
        }
        inner.events_on_disk += 1;
        if kind == "unlock_failed" {
            let window = inner.failures.entry(share.to_string()).or_default();
            prune_failures(window, now);
            window.push_back(now);
        }
        count(&mut inner.counts, &event);
        push_event(&mut inner.events, event);
        if inner.events_on_disk > EVENTS_ON_DISK_MAX {
            self.cap_events(&mut inner);
        }
    }

    /// Rewrites the events file with the events kept in memory, which are its last `EVENTS_IN_MEMORY` lines. A
    /// failure is named on stderr, and the next record tries again.
    fn cap_events(&self, inner: &mut Inner) {
        let mut text = String::new();
        for event in &inner.events {
            text.push_str(&serde_json::to_string(event).expect("a share event serializes"));
            text.push('\n');
        }
        match write_atomic(&self.events_path, text.as_bytes()) {
            Ok(()) => inner.events_on_disk = inner.events.len(),
            Err(e) => eprintln!("shares: cap {}: {e}", self.events_path.display()),
        }
    }

    /// The most recent events of share `id`, newest first, at most `limit` of them.
    pub fn events(&self, share: &str, limit: usize) -> Vec<ShareEvent> {
        self.lock()
            .events
            .iter()
            .rev()
            .filter(|e| e.share == share)
            .take(limit)
            .cloned()
            .collect()
    }

    /// The views and the downloads of share `id`, counted over all its recorded events.
    pub fn counts(&self, share: &str) -> (u64, u64) {
        self.lock().counts.get(share).copied().unwrap_or((0, 0))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Writes the shares to `shares.json`. A failure is named on stderr, and the next save tries again.
    fn save(&self, inner: &Inner) {
        if let Err(e) = write_atomic(&self.shares_path, &shares_bytes(&inner.shares)) {
            eprintln!("shares: save {}: {e}", self.shares_path.display());
        }
    }

    /// HMAC-SHA256 of `message` under the store's secret.
    fn mac(&self, message: &str) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC takes a key of any length");
        mac.update(message.as_bytes());
        let mut out = [0u8; 32];
        out.copy_from_slice(&mac.finalize().into_bytes());
        out
    }
}

/// Whether `share` has expired at `now`.
fn expired(share: &Share, now: u64) -> bool {
    share.expires_ms.is_some_and(|at| now >= at)
}

/// The shares in `path`, or none when the file is missing. A file that does not parse is moved aside as
/// `shares.json.corrupt-<unix ms>` and logged as an error, so the next save never replaces it unseen.
fn read_shares(path: &Path) -> Vec<Share> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!(
                "shares: ERROR cannot read {} ({e}); starting with no shares",
                path.display()
            );
            return Vec::new();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(shares) => shares,
        Err(e) => {
            let aside = path.with_file_name(format!("shares.json.corrupt-{}", unix_ms()));
            match std::fs::rename(path, &aside) {
                Ok(()) => eprintln!(
                    "shares: ERROR {} is not valid share JSON ({e}); moved to {}, starting with no shares",
                    path.display(),
                    aside.display()
                ),
                Err(moved) => eprintln!(
                    "shares: ERROR {} is not valid share JSON ({e}) and could not be moved aside ({moved}); \
                     starting with no shares, and the next save replaces it",
                    path.display()
                ),
            }
            Vec::new()
        }
    }
}

/// The events in `path` (the last `EVENTS_IN_MEMORY` of them), the counts of all of them, and the number of lines
/// the file holds. A line that does not parse is named on stderr and skipped.
fn read_events(path: &Path) -> (VecDeque<ShareEvent>, HashMap<String, (u64, u64)>, usize) {
    let mut events = VecDeque::new();
    let mut counts = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return (events, counts, 0);
    };
    let mut lines = 0;
    for line in text.lines() {
        lines += 1;
        match serde_json::from_str::<ShareEvent>(line) {
            Ok(event) => {
                count(&mut counts, &event);
                push_event(&mut events, event);
            }
            Err(_) => eprintln!(
                "shares: skipped a line of {} that is not an event",
                path.display()
            ),
        }
    }
    (events, counts, lines)
}

/// The pretty JSON `shares.json` holds for `shares`.
fn shares_bytes(shares: &[Share]) -> Vec<u8> {
    serde_json::to_vec_pretty(shares).expect("shares serialize")
}

/// The message an unlock token's MAC covers: the share id, its password hash and the expiry, one per line. A changed
/// password hash or expiry gives another message, so the token stops matching.
fn unlock_message(id: &str, password_hash: &str, expiry_hex: &str) -> String {
    format!("{id}\n{password_hash}\n{expiry_hex}")
}

/// Drops the failed unlocks that are `UNLOCK_WINDOW_MS` or older at `now_ms` from the front of `window`.
fn prune_failures(window: &mut VecDeque<u64>, now_ms: u64) {
    while window
        .front()
        .is_some_and(|&at| now_ms.saturating_sub(at) >= UNLOCK_WINDOW_MS)
    {
        window.pop_front();
    }
}

/// Adds one event to the views or downloads of its share. Other kinds are not counted.
fn count(counts: &mut HashMap<String, (u64, u64)>, event: &ShareEvent) {
    let entry = counts.entry(event.share.clone()).or_default();
    match event.kind.as_str() {
        "view" => entry.0 += 1,
        "download" => entry.1 += 1,
        _ => {}
    }
}

/// Keeps `event` as the newest, dropping the oldest once more than `EVENTS_IN_MEMORY` are held.
fn push_event(events: &mut VecDeque<ShareEvent>, event: ShareEvent) {
    events.push_back(event);
    while events.len() > EVENTS_IN_MEMORY {
        events.pop_front();
    }
}

/// The unlock secret in `path`: 32 random bytes, created on first use with mode 0600. A file of another length is
/// replaced, which stops the unlock tokens issued from it.
fn load_secret(path: &Path) -> [u8; 32] {
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return secret;
        }
        eprintln!(
            "shares: {} is not 32 bytes; a new secret replaces it and unlock tokens issued before stop working",
            path.display()
        );
    }
    let mut secret = [0u8; 32];
    apz_os::random::fill(&mut secret).expect("the share secret needs OS randomness");
    if let Err(e) = write_private(path, &secret) {
        eprintln!("shares: write {}: {e}", path.display());
    }
    secret
}

/// Writes `bytes` to `path` with mode 0600 on Unix.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// Writes `bytes` to `path` through a temp file and a rename, so a crash leaves the old file or the new one. The temp
/// file is synced before the rename, and the folder after it, so a save that returned is on disk.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    sync_parent(path)
}

/// Syncs the folder `path` is in, so a rename into it survives a crash.
#[cfg(unix)]
fn sync_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(dir) => std::fs::File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

/// Folders cannot be opened and synced on this platform, so only the file itself is synced.
#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Appends `line` and a newline to the events file, creating it.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

/// Hashes `password` with argon2id under a fresh random salt, as a PHC string.
fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    apz_os::random::fill(&mut salt).expect("password salts need OS randomness");
    let salt = SaltString::encode_b64(&salt).expect("a 16-byte salt encodes");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 hashes a password")
        .to_string()
}

/// Whether `password` matches the PHC string `stored`.
fn verify_password(stored: &str, password: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(hash) => Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok(),
        Err(_) => false,
    }
}

/// Compares two byte strings without stopping at the first difference. A length difference is visible.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The current time in Unix milliseconds (0 before the epoch).
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh state folder for one test.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-shares-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test folder");
        dir
    }

    fn artifact(id: &str) -> Target {
        Target {
            kind: "artifact".into(),
            id: id.into(),
        }
    }

    #[test]
    fn create_list_and_revoke() {
        let store = Store::load(&folder("create"));
        let a = store.create(artifact("art-1"), "A".into(), Options::default(), "cid-1");
        let b = store.create(artifact("art-2"), "B".into(), Options::default(), "cid-1");
        let f = store.create(
            Target {
                kind: "file".into(),
                id: "f-1".into(),
            },
            "F".into(),
            Options::default(),
            "cid-1",
        );
        assert_eq!(a.id.len(), 32);
        assert!(
            a.id.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        );
        assert_ne!(a.id, b.id);

        let ids: Vec<String> = store.list(None).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![f.id.clone(), b.id.clone(), a.id.clone()]);
        let only_a = store.list(Some(&artifact("art-1")));
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].id, a.id);
        assert_eq!(store.live().len(), 3);

        let revoked = store
            .revoke(&a.id)
            .expect("the save succeeds")
            .expect("revoke a known share");
        assert!(revoked.revoked_ms.is_some());
        assert!(matches!(
            store.access(&a.id, unix_ms(), false),
            Access::Revoked
        ));
        assert!(matches!(
            store.access("nope", unix_ms(), false),
            Access::Missing
        ));
        assert_eq!(store.live().len(), 2);
        assert!(store.revoke("nope").expect("the save succeeds").is_none());
    }

    #[test]
    fn expiry_ends_access() {
        let store = Store::load(&folder("expiry"));
        let opts = Options {
            expires_ms: Some(1_000),
            ..Options::default()
        };
        let share = store.create(artifact("art-1"), "A".into(), opts, "cid-1");
        assert!(matches!(store.access(&share.id, 999, false), Access::Ok(_)));
        assert!(matches!(
            store.access(&share.id, 1_000, false),
            Access::Expired
        ));
        assert!(store.live().is_empty(), "an expired share is not live");
    }

    #[test]
    fn password_set_verify_and_clear() {
        let store = Store::load(&folder("password"));
        let share = store.create(artifact("art-1"), "A".into(), Options::default(), "cid-1");
        assert!(matches!(
            store.access(&share.id, unix_ms(), false),
            Access::Ok(_)
        ));

        let changed = store
            .change(&share.id, Some(Some("hunter2".into())), None, None, None)
            .expect("change a known share");
        let stored = changed.password_hash.clone().unwrap_or_default();
        assert!(stored.starts_with("$argon2"), "argon2 PHC string");
        assert!(!stored.contains("hunter2"), "the password is not stored");
        assert!(matches!(
            store.access(&share.id, unix_ms(), false),
            Access::NeedsPassword
        ));
        assert!(matches!(
            store.access(&share.id, unix_ms(), true),
            Access::Ok(_)
        ));
        assert!(store.check_password(&share.id, "hunter2"));
        assert!(!store.check_password(&share.id, "wrong"));

        store
            .change(&share.id, Some(None), None, None, None)
            .expect("clear the password");
        assert!(matches!(
            store.access(&share.id, unix_ms(), false),
            Access::Ok(_)
        ));
        assert!(!store.check_password(&share.id, "hunter2"));
    }

    /// Options for a share with `password`, for the unlock token tests.
    fn locked(password: &str) -> Options {
        Options {
            password: Some(password.into()),
            ..Options::default()
        }
    }

    #[test]
    fn unlock_tokens_are_per_share_and_keyed() {
        let dir = folder("token");
        let store = Store::load(&dir);
        let now = unix_ms();
        let a = store.create(artifact("art-1"), "A".into(), locked("pw-one"), "cid-1");
        let b = store.create(artifact("art-2"), "B".into(), locked("pw-two"), "cid-1");
        let open = store.create(artifact("art-3"), "C".into(), Options::default(), "cid-1");
        let token = store
            .unlock_token(&a.id, now)
            .expect("a locked share has a token");
        assert!(store.token_ok(&a.id, &token, now));
        assert!(
            !store.token_ok(&b.id, &token, now),
            "a token is for its own share"
        );
        assert!(!store.token_ok(&a.id, "00", now));
        assert!(!store.token_ok(&a.id, "nodot", now));
        assert!(
            store.unlock_token(&open.id, now).is_none(),
            "an open share has no token"
        );

        let reloaded = Store::load(&dir);
        assert_eq!(
            reloaded.unlock_token(&a.id, now),
            Some(token.clone()),
            "the secret survives a restart"
        );
        assert!(reloaded.token_ok(&a.id, &token, now));
    }

    #[test]
    fn a_password_change_or_removal_invalidates_the_token() {
        let store = Store::load(&folder("token-rotate"));
        let now = unix_ms();
        let share = store.create(artifact("art-1"), "A".into(), locked("first-pw"), "cid-1");
        let old = store.unlock_token(&share.id, now).expect("a token");
        assert!(store.token_ok(&share.id, &old, now));

        store
            .change(&share.id, Some(Some("second-pw".into())), None, None, None)
            .expect("change the password");
        assert!(
            !store.token_ok(&share.id, &old, now),
            "a changed password ends the old unlock"
        );
        let new = store.unlock_token(&share.id, now).expect("a token");
        assert!(store.token_ok(&share.id, &new, now));

        store
            .change(&share.id, Some(None), None, None, None)
            .expect("clear the password");
        assert!(
            !store.token_ok(&share.id, &new, now),
            "a removed password ends the unlock"
        );
    }

    #[test]
    fn an_unlock_token_expires_after_seven_days() {
        let store = Store::load(&folder("token-expiry"));
        let now = 1_000_000;
        let share = store.create(artifact("art-1"), "A".into(), locked("pw"), "cid-1");
        let token = store.unlock_token(&share.id, now).expect("a token");
        assert!(store.token_ok(&share.id, &token, now + UNLOCK_TTL_MS - 1));
        assert!(
            !store.token_ok(&share.id, &token, now + UNLOCK_TTL_MS),
            "the expiry is refused"
        );
        assert!(!store.token_ok(&share.id, &token, now + 2 * UNLOCK_TTL_MS));
    }

    #[test]
    fn failed_unlocks_are_limited_per_window() {
        let store = Store::load(&folder("throttle"));
        let now = unix_ms();
        let share = store.create(artifact("art-1"), "A".into(), locked("pw"), "cid-1");
        for _ in 0..UNLOCK_LIMIT {
            assert!(!store.unlock_blocked(&share.id, now));
            store.record(&share.id, "unlock_failed", "network");
        }
        assert!(
            store.unlock_blocked(&share.id, now),
            "the attempt after the limit is refused"
        );
        assert!(
            !store.unlock_blocked(&share.id, unix_ms() + UNLOCK_WINDOW_MS),
            "the window passes and the share is open to attempts again"
        );
        assert_eq!(
            store.events(&share.id, 100).len(),
            UNLOCK_LIMIT,
            "every failure is still recorded"
        );
    }

    #[test]
    fn the_events_file_is_capped_at_its_last_ten_thousand() {
        let dir = folder("cap");
        let store = Store::load(&dir);
        let share = store.create(artifact("art-1"), "A".into(), Options::default(), "cid-1");
        for _ in 0..=EVENTS_ON_DISK_MAX {
            store.record(&share.id, "view", "network");
        }
        let text =
            std::fs::read_to_string(dir.join("share_events.jsonl")).expect("the events file");
        assert_eq!(text.lines().count(), EVENTS_IN_MEMORY);
        assert_eq!(store.counts(&share.id), (EVENTS_ON_DISK_MAX as u64 + 1, 0));
    }

    #[test]
    fn a_corrupt_shares_file_is_moved_aside_not_replaced() {
        let dir = folder("corrupt");
        std::fs::write(dir.join("shares.json"), b"{ not json").expect("write the file");
        let store = Store::load(&dir);
        assert!(store.list(None).is_empty());
        let moved: Vec<_> = std::fs::read_dir(&dir)
            .expect("read the folder")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("shares.json.corrupt-"))
            .collect();
        assert_eq!(
            moved.len(),
            1,
            "the corrupt file is kept under a corrupt-<ms> name"
        );
        assert!(
            !dir.join("shares.json").exists(),
            "the next save writes a fresh file"
        );
    }

    #[test]
    fn events_and_counts() {
        let store = Store::load(&folder("events"));
        let a = store.create(artifact("art-1"), "A".into(), Options::default(), "cid-1");
        store.record(&a.id, "view", "network");
        store.record(&a.id, "view", "relay");
        store.record(&a.id, "download", "network");
        store.record(&a.id, "unlock_failed", "network");
        assert_eq!(store.counts(&a.id), (2, 1));
        assert_eq!(store.counts("other"), (0, 0));

        let recent = store.events(&a.id, 2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].kind, "unlock_failed", "newest first");
        assert_eq!(store.events(&a.id, 100).len(), 4);
        assert!(store.events("other", 100).is_empty());
    }

    #[test]
    fn shares_and_events_survive_a_restart() {
        let dir = folder("persist");
        let store = Store::load(&dir);
        let opts = Options {
            version: Some(2),
            password: Some("pw".into()),
            expires_ms: None,
            allow_download: true,
        };
        let share = store.create(artifact("art-1"), "A".into(), opts, "cid-1");
        store.record(&share.id, "view", "network");
        drop(store);

        let store = Store::load(&dir);
        let reloaded = store.get(&share.id).expect("the share is back");
        assert_eq!(reloaded.title, "A");
        assert_eq!(reloaded.version, Some(2));
        assert!(reloaded.allow_download);
        assert_eq!(reloaded.target, artifact("art-1"));
        assert!(store.check_password(&share.id, "pw"));
        assert_eq!(store.counts(&share.id), (1, 0));
        assert_eq!(store.events(&share.id, 10).len(), 1);
        let file = std::fs::read_to_string(dir.join("shares.json")).expect("shares.json");
        assert!(!file.contains("\"pw\""), "the password is not written");
    }
}
