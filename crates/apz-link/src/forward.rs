//! The agents' traffic: a loopback HTTP server that the local vsockd uses as its `--apid`.
//! Each request must carry `x-apz-internal: <the node's internal token>` and a path under
//! `/internal/`; it is re-sent to the hub (`hub_url + path?query`) over pinned TLS, signed with
//! the node key, with the original method, body and content type, and the hub's answer
//! (status, content type, body) is passed back. Anything else is refused (403 / 404).

use std::sync::Arc;
use std::time::Duration;

use apz_http::{Request, Response};
use apz_trust::node::NodeKey;

use crate::identity::Identity;

/// Model calls are long; the hub gets this long to answer.
const HUB_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Answers one request from the local vsockd.
pub fn handle(identity: &Identity, key: &NodeKey, token: &str, request: &Request) -> Response {
    // Fail closed: an empty token never matches, even when the header is absent.
    let presented = request.header("x-apz-internal").unwrap_or("");
    if token.is_empty() || !apz_trust::ct_eq(presented.as_bytes(), token.as_bytes()) {
        return Response::error(403, "missing or wrong internal token");
    }
    if !request.path.starts_with("/internal/") {
        return Response::error(404, "not found");
    }

    let full_path = if request.query.is_empty() {
        request.path.clone()
    } else {
        format!("{}?{}", request.path, request.query)
    };
    let mut target = match identity.signed(key, &request.method, &full_path, &request.body) {
        Ok(target) => target,
        Err(error) => return Response::error(502, &error),
    };
    if let Some(content_type) = request.header("content-type") {
        target.headers.push(("content-type".to_string(), content_type.to_string()));
    }

    let body = if request.body.is_empty() { None } else { Some(request.body.as_slice()) };
    match apz_http::client::request(&target, &request.method, &full_path, body, HUB_TIMEOUT) {
        Ok(reply) => Response::new(reply.status, &reply.content_type, reply.body),
        Err(error) => Response::error(502, &error.to_string()),
    }
}

/// Serves [`handle`] on 127.0.0.1:`port` until the process ends.
pub fn serve(identity: Arc<Identity>, key: Arc<NodeKey>, token: String, port: u16) -> std::io::Result<apz_http::Handle> {
    apz_http::Server::bind(port)?.spawn(move |request| handle(&identity, &key, &token, request))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            hub_url: "https://127.0.0.1:1".into(),
            pin: "pin".into(),
            node_id: "node".into(),
            cid_base: 3,
            cid_count: 1,
            hub_name: String::new(),
        }
    }

    fn request(path: &str, headers: Vec<(String, String)>) -> Request {
        Request {
            method: "GET".into(),
            path: path.into(),
            query: String::new(),
            headers,
            body: Vec::new(),
            peer: None,
        }
    }

    #[test]
    fn forward_refuses_missing_or_wrong_token() {
        let key = NodeKey::generate();
        let missing = request("/internal/v1/agents", Vec::new());
        assert_eq!(handle(&identity(), &key, "secret", &missing).status, 403);

        let wrong = request("/internal/v1/agents", vec![("x-apz-internal".into(), "nope".into())]);
        assert_eq!(handle(&identity(), &key, "secret", &wrong).status, 403);

        let empty_token = request("/internal/v1/agents", Vec::new());
        assert_eq!(handle(&identity(), &key, "", &empty_token).status, 403);
    }

    #[test]
    fn forward_refuses_paths_outside_internal() {
        let key = NodeKey::generate();
        let outside = request("/v1/agents", vec![("x-apz-internal".into(), "secret".into())]);
        assert_eq!(handle(&identity(), &key, "secret", &outside).status, 404);
    }
}
