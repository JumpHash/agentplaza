//! The audit log: one row per tool call (token, tool, argument digest, outcome, duration). Append-only: each call is a
//! new row of `hub_mcp_call` (see `store_db.rs`), keyed by its seq, and no row is changed.

/// One audited call. Its fields are the columns of `hub_mcp_call` (prefixed `call_`), beside the row's seq.
#[derive(Debug, Clone, PartialEq, Eq, apz_store::Table)]
pub struct CallRecord {
    pub at_ms: u64,
    /// The token's ID; None for an anonymous call (open mode).
    pub token_id: Option<String>,
    /// The caller's IP for an anonymous call; None for a token's call.
    pub client_ip: Option<String>,
    pub tool: String,
    /// sha256 of the arguments as JSON. The arguments themselves are not stored.
    pub args_sha256: String,
    pub ok: bool,
    /// Why a call failed: `unknown_tool`, `rate_limited` or `tool_error`.
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Where calls are recorded.
pub trait Audit: Send + Sync {
    fn record(&self, call: CallRecord);
    fn list(&self) -> Vec<CallRecord>;
    /// The newest calls first, filtered by token and tool when given, at most `limit`.
    fn query(&self, token: Option<&str>, tool: Option<&str>, limit: usize) -> Vec<CallRecord> {
        self.list()
            .into_iter()
            .rev()
            .filter(|call| {
                token.is_none_or(|id| call.token_id.as_deref() == Some(id))
                    && tool.is_none_or(|name| call.tool == name)
            })
            .take(limit)
            .collect()
    }
}
