//! One agent's lifecycle, from the console: restart its VM, wipe it (a fresh workspace and,
//! as asked, its memory, transcript and name), or retire it (stop its VM for good and take
//! it off the roster). noded does the VM side (`/v1/runs/{run}/vms/{index}/…`); apid finds
//! the agent's run and VM from the roster (`Agent.run`, `Agent.vm`).
//!
//! POST /v1/agents/{cid}/restart                → {ok}
//! POST /v1/agents/{cid}/wipe   body WipeRequest → {ok, wiped: [what]}
//! POST /v1/agents/{cid}/retire                 → {ok}

use apz_http::{Params, Request, Response, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::App;

/// What a wipe clears. The workspace is always reset; the rest only when asked.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WipeRequest {
    #[serde(default)]
    pub memory: bool,
    #[serde(default)]
    pub transcript: bool,
    /// Forget its chosen name (it picks a new one). Its bonds are kept: the bond store
    /// has no forget operation yet.
    #[serde(default)]
    pub identity: bool,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/agents/{cid}/restart", restart_route)
        .post("/v1/agents/{cid}/wipe", wipe_route)
        .post("/v1/agents/{cid}/retire", retire_route)
}

fn restart_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    app.work.cancel(app, cid, "restarted by the owner");
    match noded_call(app, &run, vm, "restart") {
        Ok(()) => Response::json(200, &json!({ "ok": true })),
        Err(res) => res,
    }
}

fn wipe_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let wipe = match req.json::<WipeRequest>() {
        Ok(wipe) => wipe,
        Err(err) => return Response::error(400, &format!("bad body: {err}")),
    };
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    app.work.cancel(app, cid, "wiped by the owner");
    app.work.clear(cid);
    if let Err(res) = noded_call(app, &run, vm, "wipe") {
        return res;
    }
    let mut wiped = vec!["workspace"];
    if wipe.memory {
        app.memory.wipe(cid);
        wiped.push("memory");
    }
    if wipe.transcript {
        app.transcripts.clear(cid);
        app.db.delete_transcript(cid);
        wiped.push("transcript");
    }
    if wipe.identity {
        if let Err(err) = app.names.forget(cid) {
            return Response::error(500, &format!("could not forget the name of agent {cid}: {err}"));
        }
        wiped.push("identity");
    }
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
    Response::json(200, &json!({ "ok": true, "wiped": wiped }))
}

fn retire_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    app.work.cancel(app, cid, "retired by the owner");
    app.work.clear(cid);
    if let Err(res) = noded_call(app, &run, vm, "stop") {
        return res;
    }
    crate::agents::gone(app, cid, "retired by the owner");
    app.agents.remove(cid);
    Response::json(200, &json!({ "ok": true }))
}

fn cid_of(p: &Params<'_>) -> Result<u64, Response> {
    p.get("cid").parse::<u64>().map_err(|_| Response::error(400, "bad agent id"))
}

/// The run and VM index the agent sits on; 404 if unknown, 409 if it has no VM here.
fn place(app: &App, cid: u64) -> Result<(String, u32), Response> {
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(404, &format!("no agent {cid}")));
    };
    match (agent.run, agent.vm) {
        (Some(run), Some(vm)) => Ok((run, vm)),
        _ => Err(Response::error(409, &format!("agent {cid} has no VM on this node"))),
    }
}

/// Asks noded to run `action` on the VM; any failure becomes the response.
fn noded_call(app: &App, run: &str, vm: u32, action: &str) -> Result<(), Response> {
    let path = format!("/v1/runs/{run}/vms/{vm}/{action}");
    match app.noded.post(&path, &json!({})) {
        Ok((code, _)) if (200..300).contains(&code) => Ok(()),
        Ok((code, body)) => Err(Response::error(502, &format!("noded {action} failed: {code} {body}"))),
        Err(err) => Err(Response::error(502, &format!("noded unreachable: {err}"))),
    }
}
