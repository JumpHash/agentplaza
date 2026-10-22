//! Share links (docs/design/sharing.md): "anyone with the link" for an artifact or a Library file, served by this
//! hub at `{hub}/artifact/{id}` and, through the agentplaza.io relay, at `https://agentplaza.io/artifact/{id}`.
//!
//! The shares live in `plaza.db` (`share`, keyed by the link token) and their events in `share_event` (keyed by seq),
//! both as typed rows. The store keeps nothing in memory: each call reads or writes the database. The HMAC key for
//! unlock tokens is not a record and stays in `<state_dir>/share_secret`.

pub mod public;
pub mod relay;
pub mod request;
pub mod routes;

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_store::rusqlite::{Connection, Params, params};
use apz_store::table::{Changes, Loaded, Row, Value, apply};
use apz_store::{Db, IndexDef, RowSet, StoreError, Table, TableDef};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::store::sql;

/// The failed unlocks of one share allowed in `UNLOCK_WINDOW_MS`. The next attempt is refused without running argon2.
pub const UNLOCK_LIMIT: usize = 10;

/// The window in which failed unlocks of one share are counted: 10 minutes, in milliseconds.
pub const UNLOCK_WINDOW_MS: u64 = 600_000;

/// How long an unlock token lasts: 7 days, in milliseconds.
pub const UNLOCK_TTL_MS: u64 = 604_800_000;

/// What a share points at. `kind` is "artifact" or "file"; `id` is the artifact's id or the Library file's id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Table)]
pub struct Target {
    pub kind: String,
    pub id: String,
}

/// One share link (a row of `share`). Its place in the table is its creation order.
#[derive(Clone, Debug, Serialize, Deserialize, Table)]
#[table(name = "share")]
pub struct Share {
    /// 32 lowercase hex characters: 128 random bits, the token in the link.
    #[table(key)]
    pub id: String,
    #[table(flatten)]
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

/// One recorded use of a share (a row of `share_event`). Its key is its place in the log.
#[derive(Clone, Debug, Serialize, Deserialize, Table)]
#[table(name = "share_event")]
pub struct ShareEvent {
    /// The event's place in the log, assigned when it is written.
    #[serde(skip)]
    #[table(key)]
    pub seq: u64,
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

/// The share store over `plaza.db`, with the unlock secret of its folder.
pub struct Store {
    db: Arc<Db>,
    secret: [u8; 32],
    /// A store made by `for_tests` keeps its folder and its hub's store alive with it.
    #[cfg(test)]
    _keep: Option<(tempfile::TempDir, apz_store::Store)>,
}

impl Store {
    /// The store over `db` (the hub's `plaza.db`). The unlock secret is `<state_dir>/share_secret`, created on first
    /// use.
    pub fn open(db: Arc<Db>, state_dir: &Path) -> Store {
        Store {
            db,
            secret: load_secret(&state_dir.join("share_secret")),
            #[cfg(test)]
            _keep: None,
        }
    }

    /// A store in a fresh folder, kept alive with the store (the folder goes when the store drops).
    #[cfg(test)]
    pub fn for_tests() -> Store {
        let dir = tempfile::Builder::new()
            .prefix("apid-shares")
            .tempdir()
            .expect("a temporary folder");
        let store = crate::store::open_for_tests(dir.path()).expect("the store opens");
        let mut shares = Store::open(Arc::clone(store.db(crate::store::PLAZA)), dir.path());
        shares._keep = Some((dir, store));
        shares
    }

    /// Creates a share of `target`, saves it and returns it. The id is 128 random bits. A password is kept as an
    /// argon2 hash. The save waits for its commit: a failure is returned and no share exists.
    pub fn create(
        &self,
        target: Target,
        title: String,
        opts: Options,
        created_by: &str,
    ) -> Result<Share, StoreError> {
        let mut bytes = [0u8; 16];
        apz_os::random::fill(&mut bytes).expect("share ids need OS randomness");
        let share = Share {
            id: hex(&bytes),
            target,
            version: opts.version,
            password_hash: password_hash(opts.password),
            expires_ms: opts.expires_ms,
            allow_download: opts.allow_download,
            created_by: created_by.to_string(),
            created_ms: unix_ms(),
            revoked_ms: None,
            title,
        };
        let stored = share.clone();
        self.db.write(move |conn| save_share(conn, &stored))?;
        Ok(share)
    }

    /// Changes the given fields of share `id` and saves it. The outer `Option` of each argument says whether the
    /// field changes; `Some(None)` clears it. An empty password clears it. `Ok(None)` is an unknown id.
    pub fn change(
        &self,
        id: &str,
        password: Option<Option<String>>,
        expires_ms: Option<Option<u64>>,
        allow_download: Option<bool>,
        version: Option<Option<u32>>,
    ) -> Result<Option<Share>, StoreError> {
        // The hash is made before the write, so the argon2 work does not hold up the file's writer.
        let password = password.map(password_hash);
        let id = id.to_string();
        self.db
            .write(move |conn| {
                let Some(mut share) = share_of(conn, &id)? else {
                    return Ok(None);
                };
                if let Some(password) = password {
                    share.password_hash = password;
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
                save_share(conn, &share)?;
                Ok(Some(share))
            })
            .map(|written| written.value)
    }

    /// Revokes share `id` (the first revocation time is kept) and saves it. `Ok(None)` is an unknown id. A failed
    /// save is returned, and nothing is revoked, so the owner can try again.
    pub fn revoke(&self, id: &str) -> Result<Option<Share>, StoreError> {
        let id = id.to_string();
        let now = unix_ms();
        self.db
            .write(move |conn| {
                let Some(mut share) = share_of(conn, &id)? else {
                    return Ok(None);
                };
                share.revoked_ms = share.revoked_ms.or(Some(now));
                save_share(conn, &share)?;
                Ok(Some(share))
            })
            .map(|written| written.value)
    }

    /// The share with `id`, whatever its state. A failed read is named on stderr and reads as no share.
    pub fn get(&self, id: &str) -> Option<Share> {
        let id = id.to_string();
        self.read("get", None, move |conn| share_of(conn, &id))
    }

    /// Every share, or the shares of `target` only, newest first.
    pub fn list(&self, target: Option<&Target>) -> Vec<Share> {
        let target = target.cloned();
        self.read("list", Vec::new(), move |conn| match target {
            Some(target) => read_rows(
                conn,
                sql::SHARE_LIST_TARGET,
                def(),
                params![target.kind, target.id],
            ),
            None => read_rows(conn, sql::SHARE_LIST, def(), params![]),
        })
    }

    /// The shares that are neither revoked nor expired now, newest first.
    pub fn live(&self) -> Vec<Share> {
        let now = int(unix_ms());
        self.read("live", Vec::new(), move |conn| {
            read_rows(conn, sql::SHARE_LIVE, def(), params![now])
        })
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
    /// blocked attempt must not run the password check. A failed read blocks, so the throttle fails closed.
    pub fn unlock_blocked(&self, id: &str, now_ms: u64) -> bool {
        let id = id.to_string();
        let since = int(now_ms.saturating_sub(UNLOCK_WINDOW_MS));
        self.read("unlock_blocked", true, move |conn| {
            let failed: i64 = conn
                .prepare_cached(sql::SHARE_EVENT_FAILURES)?
                .query_row(params![id, since], |r| r.get(0))?;
            Ok(usize::try_from(failed).unwrap_or(0) >= UNLOCK_LIMIT)
        })
    }

    /// Records a use of share `id` in `share_event`, and counts it when it is a view or a download. The write waits for
    /// its commit, so a failed unlock counts against the next attempt at once. A failure is named on stderr.
    pub fn record(&self, share: &str, kind: &str, via: &str) {
        let event = ShareEvent {
            seq: 0,
            share: share.to_string(),
            at_ms: unix_ms(),
            kind: kind.to_string(),
            via: via.to_string(),
        };
        let result = self.db.write(move |conn| {
            let mut event = event;
            event.seq = next_event_seq(conn)?;
            let seq = event.seq;
            save(conn, event_def(), &event, seq)
        });
        if let Err(e) = result {
            eprintln!("shares: record: {e}");
        }
    }

    /// The most recent events of share `id`, newest first, at most `limit` of them.
    pub fn events(&self, share: &str, limit: usize) -> Vec<ShareEvent> {
        let share = share.to_string();
        let limit = int(limit);
        self.read("events", Vec::new(), move |conn| {
            read_rows(
                conn,
                sql::SHARE_EVENT_PAGE,
                event_def(),
                params![share, limit],
            )
        })
    }

    /// The views and the downloads of share `id`, counted over all its recorded events.
    pub fn counts(&self, share: &str) -> (u64, u64) {
        let share = share.to_string();
        self.read("counts", (0, 0), move |conn| {
            let (views, downloads): (i64, i64) = conn
                .prepare_cached(sql::SHARE_EVENT_COUNTS)?
                .query_row(params![share], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok((
                u64::try_from(views).unwrap_or(0),
                u64::try_from(downloads).unwrap_or(0),
            ))
        })
    }

    /// Reads through one read transaction. A failed read is named on stderr and gives `fallback`, the answer that
    /// refuses (no share, no events, nothing live).
    fn read<T>(
        &self,
        what: &str,
        fallback: T,
        look: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> T {
        self.db.read(look).unwrap_or_else(|e| {
            eprintln!("shares: {what}: {e}");
            fallback
        })
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

/// The table of shares (`share`), keyed by the link token and ordered by creation.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Share)
}

/// The table of share events (`share_event`), keyed by seq, with an index on (share, at_ms) for one share's events.
pub fn event_def() -> &'static TableDef {
    static DEF: OnceLock<TableDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = ShareEvent::root();
        def.indexes.push(IndexDef {
            columns: vec!["share".to_string(), "at_ms".to_string()],
            unique: false,
        });
        def
    })
}

/// The stored form of a password: its argon2 hash, or none for an empty password.
fn password_hash(password: Option<String>) -> Option<String> {
    password
        .filter(|p| !p.is_empty())
        .map(|p| hash_password(&p))
}

/// Whether `share` has expired at `now`.
fn expired(share: &Share, now: u64) -> bool {
    share.expires_ms.is_some_and(|at| now >= at)
}

/// The share `id` as the row holds it, if there is one.
fn share_of(conn: &Connection, id: &str) -> Result<Option<Share>, StoreError> {
    Ok(
        read_rows::<Share>(conn, sql::SHARE_GET, def(), params![id])?
            .into_iter()
            .next(),
    )
}

/// The next seq of the event log: one after the newest event (writes are one at a time, so none is taken twice).
fn next_event_seq(conn: &Connection) -> Result<u64, StoreError> {
    let last: i64 = conn
        .prepare_cached(sql::SHARE_EVENT_LAST_SEQ)?
        .query_row([], |r| r.get(0))?;
    Ok(u64::try_from(last).unwrap_or(0) + 1)
}

/// Writes a share at its place: its own when it is stored already, else the next after the newest share.
fn save_share(conn: &Connection, share: &Share) -> Result<(), StoreError> {
    let pos: i64 = conn
        .prepare_cached(sql::SHARE_POS)?
        .query_row(params![share.id], |r| r.get(0))?;
    save(conn, def(), share, u64::try_from(pos).unwrap_or(0))
}

/// Writes a record and its child rows as upserts: its root row at `pos` (the table's place for the row), then every
/// child table's rows.
fn save<T: Table>(
    conn: &Connection,
    def: &TableDef,
    value: &T,
    pos: u64,
) -> Result<(), StoreError> {
    let mut rows = RowSet::new();
    value.put(def, vec![Value::Integer(int(pos))], &mut rows)?;
    let mut changes = Changes::default();
    for table in def.walk() {
        for row in rows.tables.remove(&table.name).unwrap_or_default() {
            changes.upserts.push((table.name.clone(), row));
        }
    }
    apply(conn, def, &changes)
}

/// Takes the rows `query` selects as `T`. The query selects every column of `def`'s table in the table's order, as
/// `SELECT *` does.
fn read_rows<T: Table>(
    conn: &Connection,
    query: &str,
    def: &TableDef,
    params: impl Params,
) -> Result<Vec<T>, StoreError> {
    let width = def.key.len() + def.columns.len();
    let mut stmt = conn.prepare_cached(query)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut values = Vec::with_capacity(width);
        for i in 0..width {
            values.push(r.get::<_, Value>(i)?);
        }
        let columns = values.split_off(def.key.len());
        let row = Row {
            key: values,
            columns,
        };
        out.push(T::take(def, &row, &Loaded::default())?);
    }
    Ok(out)
}

/// A count or a millisecond time as the SQL integer it is stored as (a value past `i64::MAX` is clamped).
fn int(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// The message an unlock token's MAC covers: the share id, its password hash and the expiry, one per line. A changed
/// password hash or expiry gives another message, so the token stops matching.
fn unlock_message(id: &str, password_hash: &str, expiry_hex: &str) -> String {
    format!("{id}\n{password_hash}\n{expiry_hex}")
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
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
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

    fn artifact(id: &str) -> Target {
        Target {
            kind: "artifact".into(),
            id: id.into(),
        }
    }

    /// Creates a share of `target` in `store`, which must save it.
    fn create(store: &Store, target: Target, title: &str, opts: Options) -> Share {
        store
            .create(target, title.into(), opts, "cid-1")
            .expect("the share is saved")
    }

    /// The hub's store in `dir`, and the share store over it.
    fn open(dir: &Path) -> (apz_store::Store, Store) {
        let store = crate::store::open_for_tests(dir).expect("the store opens");
        let shares = Store::open(Arc::clone(store.db(crate::store::PLAZA)), dir);
        (store, shares)
    }

    #[test]
    fn create_list_and_revoke() {
        let store = Store::for_tests();
        let a = create(&store, artifact("art-1"), "A", Options::default());
        let b = create(&store, artifact("art-2"), "B", Options::default());
        let f = create(
            &store,
            Target {
                kind: "file".into(),
                id: "f-1".into(),
            },
            "F",
            Options::default(),
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
    fn a_revocation_keeps_its_first_time() {
        let store = Store::for_tests();
        let a = create(&store, artifact("art-1"), "A", Options::default());
        let first = store
            .revoke(&a.id)
            .expect("the save succeeds")
            .expect("a known share")
            .revoked_ms;
        let again = store
            .revoke(&a.id)
            .expect("the save succeeds")
            .expect("a known share")
            .revoked_ms;
        assert!(first.is_some());
        assert_eq!(first, again);
    }

    #[test]
    fn expiry_ends_access() {
        let store = Store::for_tests();
        let opts = Options {
            expires_ms: Some(1_000),
            ..Options::default()
        };
        let share = create(&store, artifact("art-1"), "A", opts);
        assert!(matches!(store.access(&share.id, 999, false), Access::Ok(_)));
        assert!(matches!(
            store.access(&share.id, 1_000, false),
            Access::Expired
        ));
        assert!(store.live().is_empty(), "an expired share is not live");
    }

    #[test]
    fn password_set_verify_and_clear() {
        let store = Store::for_tests();
        let share = create(&store, artifact("art-1"), "A", Options::default());
        assert!(matches!(
            store.access(&share.id, unix_ms(), false),
            Access::Ok(_)
        ));

        let changed = store
            .change(&share.id, Some(Some("hunter2".into())), None, None, None)
            .expect("the save succeeds")
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
            .expect("the save succeeds")
            .expect("clear the password");
        assert!(matches!(
            store.access(&share.id, unix_ms(), false),
            Access::Ok(_)
        ));
        assert!(!store.check_password(&share.id, "hunter2"));
    }

    #[test]
    fn change_sets_the_given_fields_only() {
        let store = Store::for_tests();
        let share = create(&store, artifact("art-1"), "A", Options::default());
        let changed = store
            .change(
                &share.id,
                None,
                Some(Some(5_000)),
                Some(true),
                Some(Some(3)),
            )
            .expect("the save succeeds")
            .expect("a known share");
        assert_eq!(changed.expires_ms, Some(5_000));
        assert!(changed.allow_download);
        assert_eq!(changed.version, Some(3));
        assert!(changed.password_hash.is_none());
        let stored = store.get(&share.id).expect("the share is stored");
        assert_eq!(stored.expires_ms, Some(5_000));
        assert_eq!(stored.title, "A");
        assert!(
            store
                .change("nope", None, None, None, None)
                .expect("the save succeeds")
                .is_none(),
            "an unknown id is not changed"
        );
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
        let dir = tempfile::Builder::new()
            .prefix("apid-shares-token")
            .tempdir()
            .expect("a temporary folder");
        let (store, shares) = open(dir.path());
        let now = unix_ms();
        let a = create(&shares, artifact("art-1"), "A", locked("pw-one"));
        let b = create(&shares, artifact("art-2"), "B", locked("pw-two"));
        let open_share = create(&shares, artifact("art-3"), "C", Options::default());
        let token = shares
            .unlock_token(&a.id, now)
            .expect("a locked share has a token");
        assert!(shares.token_ok(&a.id, &token, now));
        assert!(
            !shares.token_ok(&b.id, &token, now),
            "a token is for its own share"
        );
        assert!(!shares.token_ok(&a.id, "00", now));
        assert!(!shares.token_ok(&a.id, "nodot", now));
        assert!(
            shares.unlock_token(&open_share.id, now).is_none(),
            "an open share has no token"
        );

        drop(shares);
        drop(store);
        let (_store, reloaded) = open(dir.path());
        assert_eq!(
            reloaded.unlock_token(&a.id, now),
            Some(token.clone()),
            "the secret survives a restart"
        );
        assert!(reloaded.token_ok(&a.id, &token, now));
    }

    #[test]
    fn a_password_change_or_removal_invalidates_the_token() {
        let store = Store::for_tests();
        let now = unix_ms();
        let share = create(&store, artifact("art-1"), "A", locked("first-pw"));
        let old = store.unlock_token(&share.id, now).expect("a token");
        assert!(store.token_ok(&share.id, &old, now));

        store
            .change(&share.id, Some(Some("second-pw".into())), None, None, None)
            .expect("the save succeeds")
            .expect("change the password");
        assert!(
            !store.token_ok(&share.id, &old, now),
            "a changed password ends the old unlock"
        );
        let new = store.unlock_token(&share.id, now).expect("a token");
        assert!(store.token_ok(&share.id, &new, now));

        store
            .change(&share.id, Some(None), None, None, None)
            .expect("the save succeeds")
            .expect("clear the password");
        assert!(
            !store.token_ok(&share.id, &new, now),
            "a removed password ends the unlock"
        );
    }

    #[test]
    fn an_unlock_token_expires_after_seven_days() {
        let store = Store::for_tests();
        let now = 1_000_000;
        let share = create(&store, artifact("art-1"), "A", locked("pw"));
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
        let store = Store::for_tests();
        let now = unix_ms();
        let share = create(&store, artifact("art-1"), "A", locked("pw"));
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
    fn every_event_is_kept_and_counted() {
        let store = Store::for_tests();
        let share = create(&store, artifact("art-1"), "A", Options::default());
        for _ in 0..50 {
            store.record(&share.id, "view", "network");
        }
        assert_eq!(store.counts(&share.id), (50, 0));
        assert_eq!(store.events(&share.id, 100).len(), 50);
        assert_eq!(store.events(&share.id, 3).len(), 3);
    }

    #[test]
    fn events_and_counts() {
        let store = Store::for_tests();
        let a = create(&store, artifact("art-1"), "A", Options::default());
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
    fn the_tables_are_strict_and_pass_the_schema_lint() {
        let conn = apz_store::rusqlite::Connection::open_in_memory().expect("a memory database");
        conn.execute_batch(&def().ddl()).expect("the share table");
        conn.execute_batch(&event_def().ddl())
            .expect("the event table");
        let problems = apz_store::lint::schema(&conn).expect("the schema is read");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn shares_and_events_survive_a_restart() {
        let dir = tempfile::Builder::new()
            .prefix("apid-shares-persist")
            .tempdir()
            .expect("a temporary folder");
        let password = "a-long-password-for-the-test";
        let (store, shares) = open(dir.path());
        let opts = Options {
            version: Some(2),
            password: Some(password.into()),
            expires_ms: None,
            allow_download: true,
        };
        let share = create(&shares, artifact("art-1"), "A", opts);
        shares.record(&share.id, "view", "network");
        drop(shares);
        drop(store);

        let (_store, shares) = open(dir.path());
        let reloaded = shares.get(&share.id).expect("the share is back");
        assert_eq!(reloaded.title, "A");
        assert_eq!(reloaded.version, Some(2));
        assert!(reloaded.allow_download);
        assert_eq!(reloaded.target, artifact("art-1"));
        assert!(shares.check_password(&share.id, password));
        assert_eq!(shares.counts(&share.id), (1, 0));
        assert_eq!(shares.events(&share.id, 10).len(), 1);

        let mut bytes = Vec::new();
        for entry in std::fs::read_dir(dir.path()).expect("read the folder") {
            let path = entry.expect("an entry").path();
            if path.is_file() && !path.ends_with("share_secret") {
                bytes.extend(std::fs::read(&path).expect("read a file"));
            }
        }
        assert!(
            !bytes
                .windows(password.len())
                .any(|w| w == password.as_bytes()),
            "the password is not written"
        );
    }
}
