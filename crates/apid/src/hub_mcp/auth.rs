//! Bearer tokens: minting, hashing, the token store, and the per-token rate limit.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};

use apz_http::Request;
use sha2::{Digest, Sha256};

/// Every token starts with this, so a leaked one is recognizable.
pub const PREFIX: &str = "apz_mcp_";
/// The window the rate limit counts over.
const WINDOW_MS: u64 = 60_000;

/// What a token may do. Scopes are cumulative: `Govern` includes `Operate`, which includes `Observe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Observe,
    Operate,
    Govern,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Observe => "observe",
            Scope::Operate => "operate",
            Scope::Govern => "govern",
        }
    }

    pub fn parse(text: &str) -> Option<Scope> {
        match text {
            "observe" => Some(Scope::Observe),
            "operate" => Some(Scope::Operate),
            "govern" => Some(Scope::Govern),
            _ => None,
        }
    }

    /// Whether a token of this scope may call a tool of `tool`'s scope.
    pub fn permits(self, tool: Scope) -> bool {
        tool <= self
    }

    /// Calls per minute a token of this scope may make.
    pub fn per_minute(self) -> u32 {
        match self {
            Scope::Observe => 60,
            Scope::Operate | Scope::Govern => 20,
        }
    }
}

/// One token's record. The plaintext is shown once, at creation, and never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    /// Tool names this token may call. Empty means every tool its scope permits.
    pub allowlist: Vec<String>,
    pub created_ms: u64,
    pub expires_ms: Option<u64>,
    pub last_used_ms: Option<u64>,
    pub calls: u64,
    pub revoked_at_ms: Option<u64>,
}

/// What to mint a token for.
#[derive(Debug, Clone)]
pub struct NewToken {
    pub name: String,
    pub scope: Scope,
    pub allowlist: Vec<String>,
    pub expires_ms: Option<u64>,
}

/// Why a request's bearer token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    Missing,
    Invalid,
    Revoked,
    Expired,
}

impl AuthError {
    pub fn message(self) -> &'static str {
        match self {
            AuthError::Missing => "a bearer token is required",
            AuthError::Invalid => "unknown token",
            AuthError::Revoked => "token revoked",
            AuthError::Expired => "token expired",
        }
    }
}

/// Where tokens live. Only the sha256 of a token is kept.
pub trait TokenStore: Send + Sync {
    /// Mints a token. Returns the plaintext (shown once) and the record.
    fn create(&self, new: NewToken, now: u64) -> Result<(String, TokenRecord), String>;
    /// The live record for a presented token.
    fn verify(&self, token: &str, now: u64) -> Result<TokenRecord, AuthError>;
    /// Counts one call by the token with this ID.
    fn record_use(&self, id: &str, now: u64);
    /// Revokes a token. False when it is unknown or already revoked.
    fn revoke(&self, id: &str, now: u64) -> bool;
    fn list(&self) -> Vec<TokenRecord>;
}

struct Row {
    digest: String,
    record: TokenRecord,
}

/// The in-memory store. It is lost at restart; the database table replaces it.
#[derive(Default)]
pub struct MemoryTokens {
    rows: Mutex<Vec<Row>>,
}

impl TokenStore for MemoryTokens {
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
        lock(&self.rows).push(Row { digest: digest(&token), record: record.clone() });
        Ok((token, record))
    }

    fn verify(&self, token: &str, now: u64) -> Result<TokenRecord, AuthError> {
        let wanted = digest(token);
        let rows = lock(&self.rows);
        let row = rows.iter().find(|row| row.digest == wanted).ok_or(AuthError::Invalid)?;
        if row.record.revoked_at_ms.is_some() {
            return Err(AuthError::Revoked);
        }
        if row.record.expires_ms.is_some_and(|at| now >= at) {
            return Err(AuthError::Expired);
        }
        Ok(row.record.clone())
    }

    fn record_use(&self, id: &str, now: u64) {
        if let Some(row) = lock(&self.rows).iter_mut().find(|row| row.record.id == id) {
            row.record.last_used_ms = Some(now);
            row.record.calls += 1;
        }
    }

    fn revoke(&self, id: &str, now: u64) -> bool {
        match lock(&self.rows).iter_mut().find(|row| row.record.id == id) {
            Some(row) if row.record.revoked_at_ms.is_none() => {
                row.record.revoked_at_ms = Some(now);
                true
            }
            _ => false,
        }
    }

    fn list(&self) -> Vec<TokenRecord> {
        lock(&self.rows).iter().map(|row| row.record.clone()).collect()
    }
}

/// The token a request presents in `Authorization: Bearer ...`, if any.
pub fn bearer(req: &Request) -> Option<&str> {
    let value = req.header("authorization")?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim()).filter(|token| !token.is_empty())
}

/// A new token: the prefix and 32 random bytes in base32.
pub fn mint() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    apz_os::random::fill(&mut bytes).map_err(|err| format!("cannot read random bytes: {err}"))?;
    Ok(format!("{PREFIX}{}", base32(&bytes)))
}

/// The digest a token is stored under.
pub fn digest(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `bytes` random bytes as hex.
pub fn random_hex(bytes: usize) -> Result<String, String> {
    apz_os::random::hex(bytes).map_err(|err| format!("cannot read random bytes: {err}"))
}

/// RFC 4648 base32, lowercase, without padding.
fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Locks a mutex, reading through a poisoned one (the data is still consistent: every edit is one push or one field).
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Calls per token over the last minute: observe 60, operate and govern 20.
#[derive(Default)]
pub struct RateLimiter {
    calls: Mutex<HashMap<String, VecDeque<u64>>>,
}

impl RateLimiter {
    /// Counts one call by the token with this ID. `Err` carries how many milliseconds until a call is allowed.
    pub fn take(&self, token_id: &str, scope: Scope, now: u64) -> Result<(), u64> {
        let mut calls = lock(&self.calls);
        let window = calls.entry(token_id.to_string()).or_default();
        while window.front().is_some_and(|&at| now.saturating_sub(at) >= WINDOW_MS) {
            window.pop_front();
        }
        if window.len() >= scope.per_minute() as usize {
            let oldest = window.front().copied().unwrap_or(now);
            return Err((oldest + WINDOW_MS).saturating_sub(now));
        }
        window.push_back(now);
        Ok(())
    }
}
