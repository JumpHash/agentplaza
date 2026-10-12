//! The Rules view's API: the plaza's laws (JHRL) explained, with how often each was enforced,
//! and the infractions — the acts a law refused — per agent and plaza-wide.
//!
//! GET /v1/laws                      → [LawInfo + {refusals: n, agents: [{cid, name, count}], last_at_ms}]
//! GET /v1/infractions?limit=        → [Infraction + {law_title, statement, agent_name}], newest first
//! GET /v1/agents/{cid}/infractions  → the same, for one agent (all of them, not only 30 days)

use apz_http::{Params, Request, Response, Router};

use crate::state::App;

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/laws", laws_route)
        .get("/v1/infractions", infractions_route)
        .get("/v1/agents/{cid}/infractions", agent_infractions_route)
}

fn laws_route(_app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: J2")
}

fn infractions_route(_app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: J2")
}

fn agent_infractions_route(_app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: J2")
}
