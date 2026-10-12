//! One agent's lifecycle, from the console: restart its VM, wipe it (a fresh workspace and,
//! as asked, its memory, transcript and name), or retire it (stop its VM for good and take
//! it off the roster). noded does the VM side (`/v1/runs/{run}/vms/{index}/…`); apid finds
//! the agent's run and VM from the roster (`Agent.run`, `Agent.vm`).
//!
//! POST /v1/agents/{cid}/restart                → {ok, agent}
//! POST /v1/agents/{cid}/wipe   body WipeRequest → {ok, wiped: [what]}
//! POST /v1/agents/{cid}/retire                 → {ok}

use apz_http::{Params, Request, Response, Router};
use serde::Deserialize;

use crate::state::App;

/// What a wipe clears. The workspace is always reset; the rest only when asked.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WipeRequest {
    #[serde(default)]
    pub memory: bool,
    #[serde(default)]
    pub transcript: bool,
    /// Forget its chosen name (it picks a new one) and its contacts and reputation.
    #[serde(default)]
    pub identity: bool,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/agents/{cid}/restart", restart_route)
        .post("/v1/agents/{cid}/wipe", wipe_route)
        .post("/v1/agents/{cid}/retire", retire_route)
}

fn restart_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B2")
}

fn wipe_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B2")
}

fn retire_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B2")
}
