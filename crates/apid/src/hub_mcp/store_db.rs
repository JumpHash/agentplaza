//! The hub's MCP tokens and audit log in Postgres (docs/design/hub-mcp.md). They keep the rules of the in-memory
//! stores: a token is found by its digest, a revoked or expired one is refused, and a call is counted. A database
//! error on a token check refuses the token; a failed audit write is logged and the call still answers.

use std::sync::Arc;

use super::audit::{Audit, CallRecord};
use super::auth::{digest, mint, random_hex, AuthError, NewToken, TokenRecord, TokenStore};
use crate::db::Db;

/// Tokens in the `hub_mcp_tokens` table.
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
        self.db.hub_mcp_token_insert(&record, &digest(&token))?;
        Ok((token, record))
    }

    fn verify(&self, token: &str, now: u64) -> Result<TokenRecord, AuthError> {
        let record = match self.db.hub_mcp_token_by_digest(&digest(token)) {
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
        if let Err(err) = self.db.hub_mcp_token_use(id, now) {
            eprintln!("hub-mcp: counting a call by {id} failed: {err}");
        }
    }

    fn revoke(&self, id: &str, now: u64) -> bool {
        self.db.hub_mcp_token_revoke(id, now).unwrap_or_else(|err| {
            eprintln!("hub-mcp: revoking {id} failed: {err}");
            false
        })
    }

    fn list(&self) -> Vec<TokenRecord> {
        self.db.hub_mcp_tokens().unwrap_or_else(|err| {
            eprintln!("hub-mcp: listing tokens failed: {err}");
            Vec::new()
        })
    }
}

/// The audit log in the append-only `hub_mcp_calls` table.
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
        if let Err(err) = self.db.hub_mcp_call_insert(&call) {
            eprintln!("hub-mcp: audit write for {} failed: {err}", call.tool);
        }
    }

    fn list(&self) -> Vec<CallRecord> {
        self.query(None, None, 500)
    }

    fn query(&self, token: Option<&str>, tool: Option<&str>, limit: usize) -> Vec<CallRecord> {
        self.db.hub_mcp_calls(token, tool, limit).unwrap_or_else(|err| {
            eprintln!("hub-mcp: reading the call log failed: {err}");
            Vec::new()
        })
    }
}
