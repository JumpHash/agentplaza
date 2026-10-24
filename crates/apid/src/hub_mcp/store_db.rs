//! The hub's MCP tables in `plaza.db` (docs/design/hub-mcp.md): the token records, their digests, the audit log and the
//! settings row. They keep the rules of the in-memory stores they replaced: a token is found by its digest, a revoked
//! or expired one is refused, and a call is counted. A database error on a token check refuses the token; a failed
//! audit write is logged and the call still answers.
//!
//! Everything is written through as it happens: a token's record and its digest, a call and a settings save wait for
//! their commit, and each read goes to the database. The token table holds one row per token ever minted (a few dozen
//! at most), so a lookup reads it whole.

use std::sync::Arc;

use apz_store::rusqlite::{Connection, OptionalExtension, params};
use apz_store::table::{self, Changes, Loaded, Row, Value as SqlValue};
use apz_store::{Db, RowSet, StoreError, Table, TableDef};

use super::Settings;
use super::audit::{Audit, CallRecord};
use super::auth::{AuthError, NewToken, TokenRecord, TokenStore, digest, mint, random_hex};
use crate::store::sql;

/// A token's digest, in `hub_mcp_token_digest`: the sha256 of the token, keyed by the record's ID. The token itself is
/// never kept, and the token listing never reads the digests.
#[derive(Debug, Clone, PartialEq, Eq, Table)]
#[table(name = "hub_mcp_token_digest")]
struct TokenDigest {
    #[table(key)]
    token_id: String,
    #[table(unique)]
    digest: Vec<u8>,
}

/// One audited call as a row of `hub_mcp_call`: keyed by its seq, with the call's fields as its columns.
#[derive(Debug, Clone, PartialEq, Eq, Table)]
#[table(name = "hub_mcp_call")]
struct StoredCall {
    #[table(key)]
    seq: u64,
    #[table(flatten)]
    call: CallRecord,
}

/// The token records (`hub_mcp_token`, with its allowlist), written through.
pub fn token_def() -> &'static TableDef {
    apz_store::root_def!(TokenRecord)
}

/// The token digests (`hub_mcp_token_digest`), written through.
pub fn digest_def() -> &'static TableDef {
    apz_store::root_def!(TokenDigest)
}

/// The audit log (`hub_mcp_call`), written through.
pub fn call_def() -> &'static TableDef {
    apz_store::root_def!(StoredCall)
}

/// The settings row (`hub_mcp_settings`, with its allowed origins), written through.
pub fn settings_def() -> &'static TableDef {
    apz_store::root_def!(Settings)
}

/// Tokens over `hub_mcp_token` and `hub_mcp_token_digest` in `plaza.db`.
pub struct DbTokens {
    db: Arc<Db>,
}

impl DbTokens {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl TokenStore for DbTokens {
    fn create(&self, new: NewToken, now: u64) -> Result<(String, TokenRecord), String> {
        let token = mint()?;
        let record = TokenRecord {
            id: format!("mcp_{}", random_hex(6)?),
            name: new.name,
            scope: new.scope,
            allowlist: new.allowlist,
            created_ms: now,
            expires_ms: new.expires_ms,
            last_used_ms: None,
            calls: 0,
            revoked_at_ms: None,
        };
        let secret = TokenDigest {
            token_id: record.id.clone(),
            digest: digest(&token),
        };
        let row = record.clone();
        self.db
            .write(move |conn| {
                let pos = next_token_pos(conn)?;
                save(conn, token_def(), &row, pos)?;
                save(conn, digest_def(), &secret, pos)
            })
            .map_err(|err| err.to_string())?;
        Ok((token, record))
    }

    fn verify(&self, token: &str, now: u64) -> Result<TokenRecord, AuthError> {
        let wanted = digest(token);
        let record = match self.db.read(move |conn| find_token(conn, &wanted)) {
            Ok(Some(record)) => record,
            Ok(None) => return Err(AuthError::Invalid),
            Err(err) => {
                eprintln!("hub-mcp: token check failed, refusing the call: {err}");
                return Err(AuthError::Invalid);
            }
        };
        if record.revoked_at_ms.is_some() {
            return Err(AuthError::Revoked);
        }
        if record.expires_ms.is_some_and(|at| now >= at) {
            return Err(AuthError::Expired);
        }
        Ok(record)
    }

    fn record_use(&self, id: &str, now: u64) {
        let (owned, now) = (id.to_string(), millis(now));
        if let Err(err) = self.db.write(move |conn| {
            conn.execute(sql::HUB_MCP_TOKEN_USE, params![owned, now])?;
            Ok(())
        }) {
            eprintln!("hub-mcp: counting a call by {id} failed: {err}");
        }
    }

    fn revoke(&self, id: &str, now: u64) -> bool {
        let (owned, now) = (id.to_string(), millis(now));
        match self.db.write(move |conn| {
            Ok(conn.execute(sql::HUB_MCP_TOKEN_REVOKE, params![owned, now])? > 0)
        }) {
            Ok(written) => written.value,
            Err(err) => {
                eprintln!("hub-mcp: revoking {id} failed: {err}");
                false
            }
        }
    }

    fn list(&self) -> Vec<TokenRecord> {
        self.db
            .read(|conn| {
                let mut tokens = table::load_list::<TokenRecord>(conn, token_def())?;
                tokens.sort_by(|a, b| (b.created_ms, &b.id).cmp(&(a.created_ms, &a.id)));
                Ok(tokens)
            })
            .unwrap_or_else(|err| {
                eprintln!("hub-mcp: listing tokens failed: {err}");
                Vec::new()
            })
    }
}

/// The audit log over `hub_mcp_call` in `plaza.db`.
pub struct DbAudit {
    db: Arc<Db>,
}

impl DbAudit {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl Audit for DbAudit {
    fn record(&self, call: CallRecord) {
        let tool = call.tool.clone();
        let written = self.db.write(move |conn| {
            let seq = next_call_seq(conn)?;
            save(conn, call_def(), &StoredCall { seq, call }, seq)
        });
        if let Err(err) = written {
            eprintln!("hub-mcp: audit write for {tool} failed: {err}");
        }
    }

    fn list(&self) -> Vec<CallRecord> {
        self.query(None, None, 500)
    }

    fn query(&self, token: Option<&str>, tool: Option<&str>, limit: usize) -> Vec<CallRecord> {
        let token = token.map(str::to_string);
        let tool = tool.map(str::to_string);
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.db
            .read(move |conn| {
                read_rows::<StoredCall>(
                    conn,
                    sql::HUB_MCP_CALL_PAGE,
                    call_def(),
                    params![token, tool, limit],
                )
            })
            .map(|rows| rows.into_iter().map(|row| row.call).collect())
            .unwrap_or_else(|err| {
                eprintln!("hub-mcp: reading the call log failed: {err}");
                Vec::new()
            })
    }
}

/// The settings row as stored: the default (the server off, secure, local) until the owner first saves.
pub fn settings(db: &Db) -> Result<Settings, StoreError> {
    db.read(|conn| table::load_one::<Settings>(conn, settings_def()))
        .map(|found| found.unwrap_or_else(Settings::off))
}

/// Replaces the settings row with `next`, and waits for the commit.
pub fn save_settings(db: &Db, next: &Settings) -> Result<(), StoreError> {
    let rows = RowSet::one(settings_def(), next)?;
    db.write(move |conn| table::replace(conn, settings_def(), &rows))
        .map(|_| ())
}

/// The record of the token whose digest is `hash`, if one is stored.
fn find_token(conn: &Connection, hash: &[u8]) -> Result<Option<TokenRecord>, StoreError> {
    let id: Option<String> = conn
        .prepare_cached(sql::HUB_MCP_TOKEN_BY_DIGEST)?
        .query_row([hash], |r| r.get(0))
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    let found = table::load_list::<TokenRecord>(conn, token_def())?
        .into_iter()
        .find(|token| token.id == id);
    Ok(found)
}

/// The position a new token takes in the token list.
fn next_token_pos(conn: &Connection) -> Result<u64, StoreError> {
    let next: i64 = conn
        .prepare_cached(sql::HUB_MCP_TOKEN_NEXT_POS)?
        .query_row([], |r| r.get(0))?;
    Ok(u64::try_from(next).unwrap_or(0))
}

/// The seq the next call takes: one past the newest.
fn next_call_seq(conn: &Connection) -> Result<u64, StoreError> {
    let last: i64 = conn
        .prepare_cached(sql::HUB_MCP_CALL_LAST_SEQ)?
        .query_row([], |r| r.get(0))?;
    Ok(u64::try_from(last).unwrap_or(0) + 1)
}

/// Writes one new row of a root table, its children with it. `pos` is the row's place in the table's list.
fn save<T: Table>(
    conn: &Connection,
    def: &TableDef,
    value: &T,
    pos: u64,
) -> Result<(), StoreError> {
    let pos = i64::try_from(pos).unwrap_or(i64::MAX);
    let mut rows = RowSet::new();
    value.put(def, vec![SqlValue::Integer(pos)], &mut rows)?;
    let mut changes = Changes::default();
    changes.upserts = rows
        .tables
        .into_iter()
        .flat_map(|(table, rows)| rows.into_iter().map(move |row| (table.clone(), row)))
        .collect();
    table::apply(conn, def, &changes)
}

/// Reads the rows a query selects (every column of `def`'s table, in its order) as `T`.
fn read_rows<T: Table>(
    conn: &Connection,
    query: &str,
    def: &TableDef,
    params: impl apz_store::rusqlite::Params,
) -> Result<Vec<T>, StoreError> {
    let width = def.key.len() + def.columns.len();
    let mut stmt = conn.prepare_cached(query)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut values = Vec::with_capacity(width);
        for i in 0..width {
            values.push(r.get::<_, SqlValue>(i)?);
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

/// A count or a time as a SQL parameter (values past i64 are clamped; no row holds them).
fn millis(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The hub's store in a fresh folder, every file migrated the way the hub opens it. The folder is returned with the
/// store: dropping either one stops the writers.
#[cfg(test)]
pub(crate) fn test_store(label: &str) -> (tempfile::TempDir, apz_store::Store) {
    let dir = tempfile::Builder::new().prefix(label).tempdir().unwrap();
    let store = crate::store::open(dir.path()).expect("the store opens");
    (dir, store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_mcp::auth::{Scope, sha256_hex};
    use crate::hub_mcp::{AuthMode, Bind, HubMcp};

    const T0: u64 = 1_800_000_000_000;

    fn create(
        tokens: &DbTokens,
        name: &str,
        scope: Scope,
        expires_ms: Option<u64>,
    ) -> (String, TokenRecord) {
        tokens
            .create(
                NewToken {
                    name: name.into(),
                    scope,
                    allowlist: vec![],
                    expires_ms,
                },
                T0,
            )
            .expect("create")
    }

    fn call(token_id: Option<&str>, tool: &str, at_ms: u64) -> CallRecord {
        CallRecord {
            at_ms,
            token_id: token_id.map(str::to_string),
            client_ip: None,
            tool: tool.into(),
            args_sha256: sha256_hex(b"{}"),
            ok: true,
            error: None,
            duration_ms: 3,
        }
    }

    #[test]
    fn a_token_is_stored_by_its_digest_and_verifies() {
        let (dir, store) = test_store("hub-mcp-verify");
        let db = Arc::clone(store.db(crate::store::PLAZA));
        let tokens = DbTokens::new(Arc::clone(&db));
        let (token, record) = create(&tokens, "ci", Scope::Operate, None);
        assert!(token.starts_with("apz_mcp_"));
        let found = db
            .read(|conn| find_token(conn, &digest(&token)))
            .expect("query");
        assert_eq!(found.map(|t| t.id), Some(record.id.clone()));
        assert_eq!(tokens.verify(&token, T0 + 1), Ok(record));
        assert_eq!(
            tokens.verify("apz_mcp_not-a-token", T0),
            Err(AuthError::Invalid)
        );
        let on_disk: Vec<u8> = [dir.path().join("plaza.db"), dir.path().join("plaza.db-wal")]
            .iter()
            .filter_map(|path| std::fs::read(path).ok())
            .flatten()
            .collect();
        assert!(
            !on_disk.windows(token.len()).any(|w| w == token.as_bytes()),
            "the token itself is never written"
        );
    }

    #[test]
    fn a_revoked_token_is_refused_and_an_expired_one_too() {
        let (_dir, store) = test_store("hub-mcp-revoke");
        let tokens = DbTokens::new(Arc::clone(store.db(crate::store::PLAZA)));
        let (token, record) = create(&tokens, "old", Scope::Observe, None);
        assert!(tokens.revoke(&record.id, T0 + 1));
        assert!(
            !tokens.revoke(&record.id, T0 + 2),
            "a second revoke finds nothing live"
        );
        assert_eq!(tokens.verify(&token, T0 + 3), Err(AuthError::Revoked));

        let (short, _) = create(&tokens, "short", Scope::Observe, Some(T0 + 1_000));
        assert!(tokens.verify(&short, T0 + 999).is_ok());
        assert_eq!(tokens.verify(&short, T0 + 1_000), Err(AuthError::Expired));
    }

    #[test]
    fn calls_are_counted_and_audited_newest_first() {
        let (_dir, store) = test_store("hub-mcp-audit");
        let db = Arc::clone(store.db(crate::store::PLAZA));
        let tokens = DbTokens::new(Arc::clone(&db));
        let audit = DbAudit::new(Arc::clone(&db));
        let (_, record) = create(&tokens, "reader", Scope::Observe, None);
        tokens.record_use(&record.id, T0 + 5);
        let first = CallRecord {
            at_ms: T0 + 6,
            ..call(Some(&record.id), "hub_status", T0 + 6)
        };
        audit.record(first.clone());
        audit.record(CallRecord {
            at_ms: T0 + 7,
            ok: false,
            error: Some("rate_limited".into()),
            ..first.clone()
        });

        let counted = tokens
            .list()
            .into_iter()
            .find(|t| t.id == record.id)
            .expect("listed");
        assert_eq!((counted.calls, counted.last_used_ms), (1, Some(T0 + 5)));
        let rows = audit.query(Some(&record.id), Some("hub_status"), 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].error.as_deref(),
            Some("rate_limited"),
            "newest first"
        );
        assert_eq!(rows[1], first);
        assert!(audit.query(None, Some("other_tool"), 10).is_empty());
        assert_eq!(audit.query(None, None, 1).len(), 1, "the limit applies");
        assert_eq!(db.read(next_call_seq).expect("seq"), 3);
    }

    #[test]
    fn the_token_list_is_newest_first_with_its_tools_in_order() {
        let (_dir, store) = test_store("hub-mcp-list");
        let tokens = DbTokens::new(Arc::clone(store.db(crate::store::PLAZA)));
        let tools = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let (_, older) = tokens
            .create(
                NewToken {
                    name: "older".into(),
                    scope: Scope::Observe,
                    allowlist: tools(&["hub_status", "hub_agents"]),
                    expires_ms: None,
                },
                T0,
            )
            .expect("create");
        let (_, newer) = tokens
            .create(
                NewToken {
                    name: "newer".into(),
                    scope: Scope::Operate,
                    allowlist: vec![],
                    expires_ms: None,
                },
                T0 + 1,
            )
            .expect("create");
        let listed = tokens.list();
        assert_eq!(listed, vec![newer, older.clone()]);
        assert_eq!(listed[1].allowlist, tools(&["hub_status", "hub_agents"]));
    }

    #[test]
    fn the_settings_row_is_the_default_until_saved() {
        let (_dir, store) = test_store("hub-mcp-settings");
        let db = store.db(crate::store::PLAZA);
        let default = Settings {
            enabled: false,
            bind: Bind::Local,
            allowed_origins: vec![],
            auth_mode: AuthMode::Secure,
            open_scope: Scope::Observe,
        };
        assert_eq!(settings(db).expect("settings"), default);
        let next = Settings {
            enabled: true,
            bind: Bind::Remote,
            allowed_origins: vec!["https://console.test".into()],
            auth_mode: AuthMode::Open,
            open_scope: Scope::Operate,
        };
        save_settings(db, &next).expect("save");
        assert_eq!(settings(db).expect("settings"), next.clone());
        save_settings(
            db,
            &Settings {
                allowed_origins: vec![],
                ..next
            },
        )
        .expect("save");
        assert_eq!(
            settings(db).expect("settings").allowed_origins,
            Vec::<String>::new(),
            "a save replaces the origins too"
        );
    }

    #[test]
    fn tokens_calls_and_settings_survive_a_reopen() {
        let dir = tempfile::Builder::new()
            .prefix("hub-mcp-reopen")
            .tempdir()
            .unwrap();
        let settings_saved = Settings {
            enabled: true,
            bind: Bind::Local,
            allowed_origins: vec!["https://console.test".into()],
            auth_mode: AuthMode::Secure,
            open_scope: Scope::Observe,
        };
        let token = {
            let store = crate::store::open(dir.path()).expect("the store opens");
            let db = Arc::clone(store.db(crate::store::PLAZA));
            let tokens = DbTokens::new(Arc::clone(&db));
            let (token, record) = create(&tokens, "laptop", Scope::Operate, Some(T0 + 99_999));
            tokens.record_use(&record.id, T0 + 5);
            DbAudit::new(Arc::clone(&db)).record(call(Some(&record.id), "hub_status", T0 + 6));
            save_settings(&db, &settings_saved).expect("save");
            token
        };

        let store = crate::store::open(dir.path()).expect("the store opens again");
        let db = Arc::clone(store.db(crate::store::PLAZA));
        let tokens = DbTokens::new(Arc::clone(&db));
        let listed = tokens.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (
                listed[0].name.as_str(),
                listed[0].calls,
                listed[0].last_used_ms
            ),
            ("laptop", 1, Some(T0 + 5))
        );
        assert_eq!(listed[0].expires_ms, Some(T0 + 99_999));
        assert!(tokens.verify(&token, T0 + 6).is_ok(), "the digest survives");
        let calls = DbAudit::new(Arc::clone(&db)).query(None, None, 10);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "hub_status");
        assert_eq!(settings(&db).expect("settings"), settings_saved.clone());
        let hub = HubMcp::open(vec![], false, Arc::clone(&db));
        assert_eq!(
            hub.settings(),
            settings_saved,
            "the server starts with the saved settings"
        );
        assert!(hub.enabled());
    }
}
