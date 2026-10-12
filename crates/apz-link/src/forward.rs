//! The agents' traffic: a loopback HTTP server that the local vsockd uses as its `--apid`.
//! Each request must carry `x-apz-internal: <the node's internal token>` and a path under
//! `/internal/`; it is re-sent to the hub (`hub_url + path?query`) over pinned TLS, signed with
//! the node key, with the original method, body and content type, and the hub's answer
//! (status, content type, body) is passed back. Anything else is refused (403 / 404).

use std::sync::Arc;

use apz_http::{Request, Response};
use apz_trust::node::NodeKey;

use crate::identity::Identity;

/// Answers one request from the local vsockd.
pub fn handle(identity: &Identity, key: &NodeKey, token: &str, request: &Request) -> Response {
    let _ = (identity, key, token, request);
    Response::error(503, "contract: L4")
}

/// Serves [`handle`] on 127.0.0.1:`port` until the process ends.
pub fn serve(identity: Arc<Identity>, key: Arc<NodeKey>, token: String, port: u16) -> std::io::Result<apz_http::Handle> {
    let _ = (identity, key, token, port);
    Err(std::io::Error::other("contract: L4"))
}
