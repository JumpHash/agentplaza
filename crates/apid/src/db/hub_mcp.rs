//! The hub's MCP server tables (docs/design/hub-mcp.md): the token records, the audit log and the settings
//! row. Every statement lives in `sql/queries/hub_mcp_*.sql`. `crate::hub_mcp::store_db` adapts these to the
//! server's traits.

use postgres::Row;

use super::sql;
use super::Db;
use crate::hub_mcp::audit::CallRecord;
use crate::hub_mcp::auth::{Scope, TokenRecord};
use crate::hub_mcp::{Bind, Settings};

impl Db {
    /// The settings row (migration 0004 seeds it).
    pub(crate) fn hub_mcp_settings(&self) -> Result<Settings, String> {
        let row = self.run(|c| c.query_one(sql::HUB_MCP_SETTINGS_GET, &[]))?;
        let bind: String = row.get(1);
        let bind = Bind::parse(&bind).ok_or_else(|| format!("hub_mcp_settings: unknown bind {bind}"))?;
        let auth: String = row.get(3);
        let auth_mode = crate::hub_mcp::AuthMode::parse(&auth).ok_or_else(|| format!("hub_mcp_settings: unknown auth_mode {auth}"))?;
        let scope: String = row.get(4);
        let open_scope = Scope::parse(&scope).ok_or_else(|| format!("hub_mcp_settings: unknown open_scope {scope}"))?;
        Ok(Settings { enabled: row.get(0), bind, allowed_origins: row.get(2), auth_mode, open_scope })
    }

    pub(crate) fn hub_mcp_settings_put(&self, next: &Settings) -> Result<(), String> {
        let bind = next.bind.as_str();
        let auth = next.auth_mode.as_str();
        let scope = next.open_scope.as_str();
        self.run(|c| c.execute(sql::HUB_MCP_SETTINGS_PUT, &[&next.enabled, &bind, &next.allowed_origins, &auth, &scope]))?;
        Ok(())
    }

    pub(crate) fn hub_mcp_token_insert(&self, record: &TokenRecord, digest: &str) -> Result<(), String> {
        let scope = record.scope.as_str();
        let created = record.created_ms as i64;
        let expires = record.expires_ms.map(|at| at as i64);
        self.run(|c| {
            c.execute(
                sql::HUB_MCP_TOKEN_INSERT,
                &[&record.id, &record.name, &scope, &record.allowlist, &digest, &created, &expires],
            )
        })?;
        Ok(())
    }

    pub(crate) fn hub_mcp_token_by_digest(&self, digest: &str) -> Result<Option<TokenRecord>, String> {
        let row = self.run(|c| c.query_opt(sql::HUB_MCP_TOKEN_BY_DIGEST, &[&digest]))?;
        row.as_ref().map(token_row).transpose()
    }

    pub(crate) fn hub_mcp_tokens(&self) -> Result<Vec<TokenRecord>, String> {
        let rows = self.run(|c| c.query(sql::HUB_MCP_TOKEN_LIST, &[]))?;
        rows.iter().map(token_row).collect()
    }

    pub(crate) fn hub_mcp_token_use(&self, id: &str, now: u64) -> Result<(), String> {
        let now = now as i64;
        self.run(|c| c.execute(sql::HUB_MCP_TOKEN_USE, &[&id, &now]))?;
        Ok(())
    }

    /// Revokes a live token. False when it is unknown or already revoked.
    pub(crate) fn hub_mcp_token_revoke(&self, id: &str, now: u64) -> Result<bool, String> {
        let now = now as i64;
        Ok(self.run(|c| c.execute(sql::HUB_MCP_TOKEN_REVOKE, &[&id, &now]))? > 0)
    }

    pub(crate) fn hub_mcp_call_insert(&self, call: &CallRecord) -> Result<(), String> {
        let at = call.at_ms as i64;
        let duration = call.duration_ms as i64;
        self.run(|c| {
            c.execute(
                sql::HUB_MCP_CALL_INSERT,
                &[&call.token_id, &call.client_ip, &call.tool, &call.args_sha256, &call.ok, &call.error, &duration, &at],
            )
        })?;
        Ok(())
    }

    /// The newest calls first, filtered by token and tool when given.
    pub(crate) fn hub_mcp_calls(&self, token: Option<&str>, tool: Option<&str>, limit: usize) -> Result<Vec<CallRecord>, String> {
        let limit = limit as i64;
        let rows = self.run(|c| c.query(sql::HUB_MCP_CALL_PAGE, &[&token, &tool, &limit]))?;
        Ok(rows.iter().map(call_row).collect())
    }
}

fn token_row(row: &Row) -> Result<TokenRecord, String> {
    let scope: String = row.get(2);
    let ms = |column: usize| row.get::<_, Option<i64>>(column).map(|at| at as u64);
    Ok(TokenRecord {
        id: row.get(0),
        name: row.get(1),
        scope: Scope::parse(&scope).ok_or_else(|| format!("hub_mcp_tokens: unknown scope {scope}"))?,
        allowlist: row.get(3),
        created_ms: ms(4).unwrap_or(0),
        expires_ms: ms(5),
        last_used_ms: ms(6),
        calls: row.get::<_, i64>(7) as u64,
        revoked_at_ms: ms(8),
    })
}

fn call_row(row: &Row) -> CallRecord {
    CallRecord {
        token_id: row.get(0),
        client_ip: row.get(7),
        tool: row.get(1),
        args_sha256: row.get(2),
        ok: row.get(3),
        error: row.get(4),
        duration_ms: row.get::<_, i64>(5) as u64,
        at_ms: row.get::<_, i64>(6) as u64,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::hub_mcp::audit::Audit;
    use crate::hub_mcp::auth::{digest, AuthError, NewToken, TokenStore};
    use crate::hub_mcp::store_db::{DbAudit, DbTokens};

    const T0: u64 = 1_800_000_000_000;

    /// A fresh test database with the schema applied (skipped without APZ_TEST_DATABASE_URL).
    fn database(label: &str) -> Option<Arc<Db>> {
        let url = crate::db::test_database(label)?;
        let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
        let db = Db::connect(&env);
        assert!(db.enabled());
        Some(Arc::new(db))
    }

    fn create(tokens: &DbTokens, name: &str, scope: Scope, expires_ms: Option<u64>) -> (String, TokenRecord) {
        tokens.create(NewToken { name: name.into(), scope, allowlist: vec![], expires_ms }, T0).expect("create")
    }

    #[test]
    fn a_token_is_stored_by_its_hash_and_verifies() {
        let Some(db) = database("hub_mcp_verify") else { return };
        let tokens = DbTokens::new(Arc::clone(&db));
        let (token, record) = create(&tokens, "ci", Scope::Operate, None);
        assert!(token.starts_with("apz_mcp_"));
        let stored = db.hub_mcp_token_by_digest(&digest(&token)).expect("query").expect("stored by digest");
        assert_eq!(stored.id, record.id);
        assert_eq!(tokens.verify(&token, T0 + 1), Ok(record));
        assert_eq!(tokens.verify("apz_mcp_not-a-token", T0), Err(AuthError::Invalid));
    }

    #[test]
    fn a_revoked_token_is_refused_and_an_expired_one_too() {
        let Some(db) = database("hub_mcp_revoke") else { return };
        let tokens = DbTokens::new(Arc::clone(&db));
        let (token, record) = create(&tokens, "old", Scope::Observe, None);
        assert!(tokens.revoke(&record.id, T0 + 1));
        assert!(!tokens.revoke(&record.id, T0 + 2), "a second revoke finds nothing live");
        assert_eq!(tokens.verify(&token, T0 + 3), Err(AuthError::Revoked));

        let (short, _) = create(&tokens, "short", Scope::Observe, Some(T0 + 1_000));
        assert!(tokens.verify(&short, T0 + 999).is_ok());
        assert_eq!(tokens.verify(&short, T0 + 1_000), Err(AuthError::Expired));
    }

    #[test]
    fn calls_are_counted_and_audited_and_the_log_is_append_only() {
        let Some(db) = database("hub_mcp_audit") else { return };
        let tokens = DbTokens::new(Arc::clone(&db));
        let audit = DbAudit::new(Arc::clone(&db));
        let (_, record) = create(&tokens, "reader", Scope::Observe, None);
        tokens.record_use(&record.id, T0 + 5);
        let call = CallRecord {
            at_ms: T0 + 6,
            token_id: Some(record.id.clone()), client_ip: None,
            tool: "hub_status".into(),
            args_sha256: digest("{}"),
            ok: true,
            error: None,
            duration_ms: 3,
        };
        audit.record(call.clone());
        audit.record(CallRecord { at_ms: T0 + 7, ok: false, error: Some("rate_limited".into()), ..call.clone() });

        let counted = tokens.list().into_iter().find(|t| t.id == record.id).expect("listed");
        assert_eq!((counted.calls, counted.last_used_ms), (1, Some(T0 + 5)));
        let rows = audit.query(Some(&record.id), Some("hub_status"), 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].error.as_deref(), Some("rate_limited"), "newest first");
        assert_eq!(rows[1], call);
        assert!(audit.query(None, Some("other_tool"), 10).is_empty());

        assert!(refused(&db, "UPDATE hub_mcp_calls SET ok = false").contains("append-only"));
        assert!(refused(&db, "DELETE FROM hub_mcp_calls").contains("append-only"));
    }

    /// The server's message for a statement it refuses. `run` keeps only the error's kind, so the message is read here.
    fn refused(db: &Db, sql: &str) -> String {
        let mut message = String::new();
        let outcome = db.run(|c| {
            let result = c.execute(sql, &[]);
            if let Err(err) = &result {
                message = err.as_db_error().map_or_else(|| err.to_string(), |db| db.message().to_string());
            }
            result
        });
        assert!(outcome.is_err(), "{sql} must be refused");
        message
    }

    #[test]
    fn the_settings_row_is_seeded_and_saved() {
        let Some(db) = database("hub_mcp_settings") else { return };
        assert_eq!(db.hub_mcp_settings(), Ok(Settings { enabled: false, bind: Bind::Local, allowed_origins: vec![], auth_mode: crate::hub_mcp::AuthMode::Secure, open_scope: Scope::Observe }));
        let next = Settings { enabled: true, bind: Bind::Remote, allowed_origins: vec!["https://console.test".into()], auth_mode: crate::hub_mcp::AuthMode::Secure, open_scope: Scope::Observe };
        db.hub_mcp_settings_put(&next).expect("save");
        assert_eq!(db.hub_mcp_settings(), Ok(next));
    }
}
