//! Egress credentials: the hub adds a secret to an agent's outbound HTTPS request, so the secret never
//! enters the VM.
//!
//! The owner writes **rules**: for agents of these roles (empty = every role), a request to this host
//! (exact, or `*.suffix`) on this port, with one of these methods (empty = any) and a path under this
//! prefix, gets this header, whose value is the format with `${SECRET}` replaced by a vault secret.
//! First match wins. Rules are owner-only; the secret is named, never shown.
//!
//! How the request is reached: vsockd's egress proxy asks the hub before it dials
//! (`/internal/egress/check`), and the answer says `intercept` only when a rule could match that agent
//! and host — every other host stays an opaque CONNECT tunnel. For an intercepted host the proxy
//! terminates the guest's TLS with a leaf certificate the hub signs ([`leaf_for`], CA in
//! [`ca_for_role`]), opens its own TLS to the real host, and asks the hub per request
//! (`/internal/egress/inject`) which header to add. Each injected request is a transcript entry
//! (host, method, path, rule — never the value). CubeSandbox's egress proxy does the same with
//! OpenResty for every HTTP(S) request; here only the hosts the owner named are opened.
//!
//! GET    /v1/egress/inject            → {rules: [rule]}
//! POST   /v1/egress/inject   rule     → rule
//! PATCH  /v1/egress/inject/{id}       → rule
//! DELETE /v1/egress/inject/{id}       → {ok}
//! POST   /internal/egress/cert   {cid, host}                       → {cert_pem, key_pem}
//! POST   /internal/egress/inject {cid, host, port, method, path}   → {header, value, rule} | {}

use apz_http::Router;

use crate::state::App;

/// Routes listed in the module docs.
pub fn routes(router: Router<App>) -> Router<App> {
    router
}

/// The egress CA certificate (PEM) agents of `role` must trust: `Some` only when a rule could inject
/// for that role (see the module docs).
pub fn ca_for_role(app: &App, role: &str) -> Option<String> {
    let _ = (app, role);
    None
}

/// Whether the proxy should open agent `cid`'s connection to `host:port` (a rule could match).
pub fn intercepts(app: &App, cid: u64, host: &str, port: u16) -> bool {
    let _ = (app, cid, host, port);
    false
}
