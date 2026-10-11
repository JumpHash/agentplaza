//! The hub's internal endpoints: hello, every v3 guest line, gone.
//!
//! OWNER: slice "loop". `handle` is the agent loop's host half.

use apz_http::{Request, Response, Router};
use apz_proto::wire::{GuestMessage, HostMessage};
use serde::Deserialize;
use serde_json::json;

use crate::state::App;

#[derive(Deserialize)]
struct Hello {
    #[serde(default)]
    role: String,
    #[serde(default)]
    vm: u32,
}

#[derive(Deserialize)]
struct Gone {
    #[serde(default)]
    reason: String,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/internal/v1/agents/{cid}/hello", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let hello: Hello = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
                let profile = crate::agents::hello(app, cid, &hello.role, hello.vm);
                Ok(Response::json(200, &json!({ "profile": profile })))
            })
        })
        .post("/internal/v1/agents/{cid}/message", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let message: GuestMessage = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
                let reply = handle(app, cid, message);
                Ok(Response::json(200, &json!({ "reply": reply })))
            })
        })
        .post("/internal/v1/agents/{cid}/gone", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let gone: Gone = req.json().unwrap_or(Gone { reason: String::new() });
                crate::agents::gone(app, cid, &gone.reason);
                Ok(Response::json(200, &json!({})))
            })
        })
}

/// Checks the internal token and the CID, then runs `f`.
fn authorized(
    app: &App,
    req: &Request,
    cid: &str,
    f: impl FnOnce(u64) -> Result<Response, Response>,
) -> Response {
    if req.header("x-apz-internal") != Some(app.token.as_str()) {
        return Response::error(403, "missing or wrong internal token");
    }
    let Ok(cid) = cid.parse::<u64>() else {
        return Response::error(400, "the CID must be a number");
    };
    f(cid).unwrap_or_else(|response| response)
}

/// One v3 guest line from agent `cid`; the reply for the guest, if any.
pub fn handle(app: &App, cid: u64, message: GuestMessage) -> Option<HostMessage> {
    // contract — TODO(loop slice): model_call → gateway, tool_request → policy,
    // tool_report → transcript, work_done → scheduler::Queues::complete.
    let _ = (app, cid, message);
    None
}
