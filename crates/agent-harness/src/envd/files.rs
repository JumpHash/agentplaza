//! `GET /files` (download) and `POST /files` (multipart upload), E2B's `spec/envd/envd.yaml`.

use super::{Ctx, Reply, Request};

/// Serves `GET` or `POST /files`.
pub fn handle(request: &Request, ctx: &Ctx) -> Reply {
    let _ = (request, ctx);
    Reply::json(
        501,
        &serde_json::json!({"code": 501, "message": "files are not built yet"}),
    )
}
