//! The preview routes on a scratch hub (PLAN 6.4): the owner's routes answer, and every route that names a preview or a
//! link refuses an unknown one. A preview needs a fork on a node, which a scratch hub does not have: the preview's own
//! lifecycle, its links and its openings are tested in `crates/apid/src/previews/tests.rs` through the tunnel.

mod common;

use common::{Hub, Scratch};
use serde_json::json;

#[test]
fn the_owners_preview_routes_answer_and_refuse_what_is_not_there() {
    let scratch = Scratch::new("previews-api");
    let hub = Hub::start(&scratch, None, None);

    let (status, body) = hub.call("GET", "/v1/projects/p/previews", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["previews"], json!([]));

    let (status, _) = hub.call(
        "POST",
        "/v1/previews/nope/links",
        Some(json!({"scope": "view"})),
    );
    assert_eq!(status, 404);
    for (method, path) in [
        ("DELETE", "/v1/preview-links/nope"),
        ("GET", "/v1/preview-links/nope/openings"),
        ("GET", "/v1/preview-links/nope/qr"),
        ("GET", "/v1/previews/nope/links"),
        ("POST", "/v1/previews/nope/sleep"),
        ("POST", "/v1/previews/nope/wake"),
        ("POST", "/v1/previews/nope/close"),
    ] {
        let (status, body) = hub.call(method, path, None);
        assert_eq!(status, 404, "{method} {path}: {body}");
    }

    // The public path has no preview of that lane, so the owner's call gets the refusal the route gives.
    let (status, _) = hub.call("GET", "/preview/p/lane/", None);
    assert_eq!(status, 404);
}
