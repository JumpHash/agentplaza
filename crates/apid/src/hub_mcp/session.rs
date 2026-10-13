//! Sessions: the `Mcp-Session-Id` issued on `initialize`, bound to the token that opened it, and expired after an hour idle.

use std::collections::HashMap;
use std::sync::Mutex;

use super::auth::{lock, random_hex};

/// A session that has seen no request for this long is over.
pub const IDLE_MS: u64 = 60 * 60 * 1000;

struct Session {
    token_id: String,
    last_ms: u64,
}

/// The live sessions.
#[derive(Default)]
pub struct Sessions {
    live: Mutex<HashMap<String, Session>>,
}

/// A session that cannot be used: unknown, expired, or opened by another token. All three answer 404.
#[derive(Debug, PartialEq, Eq)]
pub struct Unknown;

impl Sessions {
    /// Opens a session for the token with this ID, and returns its ID.
    pub fn open(&self, token_id: &str, now: u64) -> Result<String, String> {
        let id = random_hex(16)?;
        let mut live = lock(&self.live);
        live.retain(|_, session| now.saturating_sub(session.last_ms) < IDLE_MS);
        live.insert(id.clone(), Session { token_id: token_id.to_string(), last_ms: now });
        Ok(id)
    }

    /// Checks that a request's session belongs to the token, and marks it used.
    pub fn touch(&self, id: &str, token_id: &str, now: u64) -> Result<(), Unknown> {
        let mut live = lock(&self.live);
        let Some(session) = live.get(id) else {
            return Err(Unknown);
        };
        if session.token_id != token_id {
            return Err(Unknown);
        }
        if now.saturating_sub(session.last_ms) >= IDLE_MS {
            live.remove(id);
            return Err(Unknown);
        }
        if let Some(session) = live.get_mut(id) {
            session.last_ms = now;
        }
        Ok(())
    }
}
