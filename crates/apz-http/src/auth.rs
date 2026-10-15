//! The machine's internal token on the node daemons' own HTTP APIs (noded, vsockd).
//!
//! A daemon binds loopback and refuses a `Host` that does not name it ([`crate::Server`]), but any
//! process on the machine can still reach it. With an [`InternalToken`], every request must also
//! carry `x-apz-internal: <token>`, the secret in a file only the plaza's own daemons read: on a hub
//! machine `<state-dir>/internal.token` (apid writes a fresh one at each start), on a joined node
//! `node/internal-token` (apz-link's). The file is read on each request, so a daemon follows a token
//! that changed under it, and a missing or empty file admits nothing.
//!
//! The callers present the same file's token: [`InternalToken::header`] gives the header to send.

use std::path::{Path, PathBuf};

use crate::{Request, Response};

/// The header that carries the token.
pub const HEADER: &str = "x-apz-internal";

/// Where a daemon (or a caller) finds the machine's internal token. `none()` is an open daemon, for
/// tests and development runs started without a token file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InternalToken(Option<PathBuf>);

impl InternalToken {
    /// The token in `path`.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self(Some(path.into()))
    }

    /// No token: every request is admitted, and callers send none.
    pub fn none() -> Self {
        Self(None)
    }

    /// The token file, if any.
    pub fn path(&self) -> Option<&Path> {
        self.0.as_deref()
    }

    /// Whether this is a token at all (a file was named).
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    /// The token as the file holds it now, trimmed; `None` when no file is named or it is
    /// unreadable or empty.
    pub fn read(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.0.as_ref()?).ok()?;
        let token = text.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    /// The header a caller sends: `Some((HEADER, token))` when a token is named and readable.
    pub fn header(&self) -> Option<(&'static str, String)> {
        self.read().map(|token| (HEADER, token))
    }

    /// Whether `request` may be served: always when no file is named; otherwise only when it
    /// carries the file's current token (compared in constant time).
    pub fn admits(&self, request: &Request) -> bool {
        if self.0.is_none() {
            return true;
        }
        match (self.read(), request.header(HEADER)) {
            (Some(token), Some(sent)) => apz_trust::ct_eq(token.as_bytes(), sent.trim().as_bytes()),
            _ => false,
        }
    }

    /// `handler` behind this token: a request without it is answered 401 and never reaches the
    /// handler.
    pub fn guard<H>(self, handler: H) -> impl Fn(&Request) -> Response + Send + Sync + 'static
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        move |request: &Request| {
            if self.admits(request) {
                handler(request)
            } else {
                Response::error(
                    401,
                    "this daemon answers only the plaza's own daemons on this machine",
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(token: Option<&str>) -> Request {
        Request {
            method: "GET".into(),
            path: "/v1/runs".into(),
            headers: token
                .map(|token| vec![(HEADER.to_string(), token.to_string())])
                .unwrap_or_default(),
            ..Request::default()
        }
    }

    fn token_file(name: &str, text: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("apz-http-auth-{name}-{}", std::process::id()));
        std::fs::write(&path, text).expect("the token file");
        path
    }

    #[test]
    fn no_file_admits_everything_and_sends_nothing() {
        let open = InternalToken::none();
        assert!(open.admits(&request(None)));
        assert!(open.header().is_none());
    }

    #[test]
    fn a_file_admits_only_its_current_token() {
        let path = token_file("current", "s3cret\n");
        let token = InternalToken::file(&path);
        assert!(token.admits(&request(Some("s3cret"))));
        assert!(!token.admits(&request(Some("wrong"))));
        assert!(!token.admits(&request(None)));
        assert_eq!(token.header(), Some((HEADER, "s3cret".to_string())));
        // A token rewritten under the daemon (apid restarted) is followed at once.
        std::fs::write(&path, "fresh").expect("rewrite");
        assert!(token.admits(&request(Some("fresh"))));
        assert!(!token.admits(&request(Some("s3cret"))));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_or_empty_file_admits_nothing() {
        let path = token_file("empty", "  \n");
        let token = InternalToken::file(&path);
        assert!(!token.admits(&request(Some(""))));
        let _ = std::fs::remove_file(&path);
        assert!(!token.admits(&request(Some("anything"))));
        assert!(token.header().is_none());
    }

    #[test]
    fn the_guard_answers_401_before_the_handler() {
        let path = token_file("guard", "s3cret");
        let guarded =
            InternalToken::file(&path).guard(|_| Response::json(200, &serde_json::json!({})));
        assert_eq!(guarded(&request(None)).status, 401);
        assert_eq!(guarded(&request(Some("s3cret"))).status, 200);
        let _ = std::fs::remove_file(&path);
    }
}
