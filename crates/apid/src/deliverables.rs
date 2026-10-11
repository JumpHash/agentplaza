//! The Library: deliverables agents hand in (files they were asked to produce), kept in the
//! plaza's object store — Garage over S3 when configured, a local folder otherwise.

use apz_http::Router;

use crate::state::App;

/// `/v1/files…` (the console) and `/internal/v1/agents/{cid}/gateway/files…` (agents, via
/// the hub's plaza.gateway).
pub fn routes(router: Router<App>) -> Router<App> {
    router
}
