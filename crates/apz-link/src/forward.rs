//! The agents' traffic: a loopback HTTP server that the local vsockd uses as its `--apid`.
//! Each request must carry `x-apz-internal: <the node's internal token>` and a path under
//! `/internal/`; it is re-sent to the hub (`hub_url + path?query`) over pinned TLS, signed with
//! the node key, with the original method, body and content type, and the hub's answer
//! (status, content type, body) is passed back. Anything else is refused (403 / 404).
//!
//! It is also the node's Weft cache: the VMs' object reads (`…/gateway/weft/wire/objects/{hash}`, `…/wire/bundle`) are
//! answered from the node's disk when it has the objects, and a miss is fetched once from the hub's node door
//! (`/internal/v1/nodes/{node}/weft/…`, signed by this node, for the asking agent). Objects are immutable, so a copy
//! is good forever; the least recently used go when the cache is full.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use apz_http::{Request, Response};
use apz_trust::node::NodeKey;

use crate::identity::Identity;

/// Model calls are long; the hub gets this long to answer.
const HUB_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The node's Weft objects.
static WEFT: OnceLock<Arc<weft::cache::Cache>> = OnceLock::new();

/// How much the node's Weft cache keeps.
const WEFT_CACHE_BYTES: u64 = 4 << 30;

/// Keeps the node's Weft objects in `dir`.
pub fn set_weft_cache(dir: &Path) -> Result<(), String> {
    let cache = weft::cache::Cache::objects(dir, WEFT_CACHE_BYTES)?;
    let _ = WEFT.set(cache);
    Ok(())
}

/// Answers a VM's Weft object read from the node's cache, fetching a miss from the hub: `None` for anything else.
fn weft_object(identity: &Identity, key: &NodeKey, request: &Request) -> Option<Response> {
    let cache = WEFT.get()?;
    let rest = request.path.strip_prefix("/internal/v1/agents/")?;
    let (cid, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("gateway/weft/wire/")?;
    let cid: u64 = cid.parse().ok()?;
    let caller = format!("agent:{cid}");
    let hub = |method: &str,
               tail: &str,
               body: Option<&[u8]>|
     -> Result<apz_http::client::Reply, String> {
        let path = format!("/internal/v1/nodes/{}/weft/wire/{tail}", identity.node_id);
        let mut target = identity.signed(key, method, &path, body.unwrap_or(&[]))?;
        target
            .headers
            .push((weft::wire::CALLER.to_string(), caller.clone()));
        if body.is_some() {
            target
                .headers
                .push(("content-type".into(), "application/json".into()));
        }
        apz_http::client::request(&target, method, &path, body, HUB_TIMEOUT)
            .map_err(|e| e.to_string())
    };
    match (request.method.as_str(), rest) {
        ("GET", tail) if tail.starts_with("objects/") && request.header("range").is_none() => {
            let hash: weft::hash::Hash = tail.strip_prefix("objects/")?.parse().ok()?;
            if let Some(bytes) = cache.get_local(&hash) {
                cache
                    .hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Some(Response::new(200, "application/octet-stream", bytes));
            }
            cache
                .misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(match hub("GET", tail, None) {
                Ok(r) if r.status == 200 && weft::hash::Hash::of(&r.body) == hash => {
                    cache.put_local(&hash, &r.body);
                    Response::new(200, "application/octet-stream", r.body)
                }
                Ok(r) => Response::new(r.status, &r.content_type, r.body),
                Err(e) => Response::error(502, &e),
            })
        }
        ("POST", "bundle") => {
            let asked: Vec<weft::hash::Hash> =
                serde_json::from_slice::<serde_json::Value>(&request.body)
                    .ok()
                    .and_then(|v| serde_json::from_value(v["hashes"].clone()).ok())?;
            let mut have = Vec::new();
            let mut lacking = Vec::new();
            for h in asked {
                match cache.get_local(&h) {
                    Some(b) => have.push((h, b)),
                    None => lacking.push(h),
                }
            }
            cache
                .hits
                .fetch_add(have.len() as u64, std::sync::atomic::Ordering::Relaxed);
            cache
                .misses
                .fetch_add(lacking.len() as u64, std::sync::atomic::Ordering::Relaxed);
            if !lacking.is_empty() {
                let body = serde_json::json!({ "hashes": lacking }).to_string();
                match hub("POST", "bundle", Some(body.as_bytes())) {
                    Ok(r) if r.status == 200 => match weft::wire::decode_bundle(&r.body) {
                        Ok(objects) => {
                            for (h, b) in objects {
                                cache.put_local(&h, &b);
                                have.push((h, b));
                            }
                        }
                        Err(e) => return Some(Response::error(502, &e)),
                    },
                    Ok(r) => return Some(Response::new(r.status, &r.content_type, r.body)),
                    Err(e) => return Some(Response::error(502, &e)),
                }
            }
            // One bundle at most; the VM asks again for the rest.
            let mut size = 0;
            let mut out = Vec::new();
            for (h, b) in have {
                if !out.is_empty() && size + b.len() > weft::wire::BUNDLE_BYTES {
                    break;
                }
                size += b.len();
                out.push((h, b));
            }
            Some(Response::new(
                200,
                "application/octet-stream",
                weft::wire::encode_bundle(&out),
            ))
        }
        _ => None,
    }
}

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
    if let Some(answer) = weft_object(identity, key, request) {
        return answer;
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
        target
            .headers
            .push(("content-type".to_string(), content_type.to_string()));
    }

    let body = if request.body.is_empty() {
        None
    } else {
        Some(request.body.as_slice())
    };
    match apz_http::client::request(&target, &request.method, &full_path, body, HUB_TIMEOUT) {
        Ok(reply) => Response::new(reply.status, &reply.content_type, reply.body),
        Err(error) => Response::error(502, &error.to_string()),
    }
}

/// Serves [`handle`] on 127.0.0.1:`port` until the process ends.
pub fn serve(
    identity: Arc<Identity>,
    key: Arc<NodeKey>,
    token: String,
    port: u16,
) -> std::io::Result<apz_http::Handle> {
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

        let wrong = request(
            "/internal/v1/agents",
            vec![("x-apz-internal".into(), "nope".into())],
        );
        assert_eq!(handle(&identity(), &key, "secret", &wrong).status, 403);

        let empty_token = request("/internal/v1/agents", Vec::new());
        assert_eq!(handle(&identity(), &key, "", &empty_token).status, 403);
    }

    #[test]
    fn the_nodes_weft_cache_fetches_a_miss_once_and_answers_the_rest_itself() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let blob = b"fn main() {}\n".to_vec();
        let hash = weft::hash::Hash::of(&blob);
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let served = blob.clone();
        let hub = apz_http::Server::bind(0)
            .unwrap()
            .spawn(move |req: &Request| {
                seen.fetch_add(1, Ordering::SeqCst);
                assert!(
                    req.path.starts_with("/internal/v1/nodes/node/weft/wire/"),
                    "{}",
                    req.path
                );
                assert_eq!(req.header(weft::wire::CALLER), Some("agent:3"));
                assert!(
                    req.header("x-apz-sig").is_some(),
                    "the node signs its misses"
                );
                if req.path.ends_with("/bundle") {
                    Response::new(
                        200,
                        "application/octet-stream",
                        weft::wire::encode_bundle(&[(
                            weft::hash::Hash::of(&served),
                            served.clone(),
                        )]),
                    )
                } else {
                    Response::new(200, "application/octet-stream", served.clone())
                }
            })
            .unwrap();
        let dir = std::env::temp_dir().join(format!("apz-link-weft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set_weft_cache(&dir).unwrap();
        let mut id = identity();
        id.hub_url = format!("http://127.0.0.1:{}", hub.port());
        id.pin = apz_trust::b64(&[0u8; 32]);
        let key = NodeKey::generate();
        let token = vec![("x-apz-internal".to_string(), "secret".to_string())];
        let get = request(
            &format!(
                "/internal/v1/agents/3/gateway/weft/wire/objects/{}",
                hash.hex()
            ),
            token.clone(),
        );
        assert_eq!(handle(&id, &key, "secret", &get).bytes(), &blob[..]);
        assert_eq!(handle(&id, &key, "secret", &get).bytes(), &blob[..]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the second read came from the node's disk"
        );
        let mut bundle = request("/internal/v1/agents/3/gateway/weft/wire/bundle", token);
        bundle.method = "POST".into();
        bundle.body = serde_json::json!({ "hashes": [hash] })
            .to_string()
            .into_bytes();
        let got = weft::wire::decode_bundle(handle(&id, &key, "secret", &bundle).bytes()).unwrap();
        assert_eq!(got, vec![(hash, blob.clone())]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the bundle came from the node's disk too"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forward_refuses_paths_outside_internal() {
        let key = NodeKey::generate();
        let outside = request(
            "/v1/agents",
            vec![("x-apz-internal".into(), "secret".into())],
        );
        assert_eq!(handle(&identity(), &key, "secret", &outside).status, 404);
    }
}
