//! One agent's lifecycle, from the console: restart its VM, wipe it (a fresh workspace and,
//! as asked, its memory, transcript and name), or retire it (stop its VM for good and take
//! it off the roster). noded does the VM side (`/v1/runs/{run}/vms/{index}/…`); apid finds
//! the agent's run and VM from the roster (`Agent.run`, `Agent.vm`).
//!
//! POST /v1/agents/{cid}/restart                → {ok}
//! POST /v1/agents/{cid}/wipe   body WipeRequest → {ok, wiped: [what]}
//! POST /v1/agents/{cid}/retire                 → {ok}

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::App;

/// What a wipe clears. The workspace is always reset; the rest only when asked.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WipeRequest {
    #[serde(default)]
    pub transcript: bool,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/agents/{cid}/restart", restart_route)
        .doc(RouteDoc::new("Restart an agent's VM, cancelling its work").errors(&[404, 409]))
        .post("/v1/agents/{cid}/wipe", wipe_route)
        .doc(
            RouteDoc::new("Wipe an agent's workspace and chosen data")
                .errors(&[400, 404, 409, 500]),
        )
        .post("/v1/agents/{cid}/retire", retire_route)
        .doc(RouteDoc::new("Retire an agent and stop its VM").errors(&[404, 409]))
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
    match noded_call(app, cid, &run, vm, "restart") {
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
    if let Err(res) = noded_call(app, cid, &run, vm, "wipe") {
        return res;
    }
    let mut wiped = vec!["workspace"];
    if wipe.transcript {
        app.transcripts.wipe(cid);
        wiped.push("transcript");
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
    if let Err(res) = noded_call(app, cid, &run, vm, "stop") {
        return res;
    }
    crate::agents::gone(app, cid, "retired by the owner");
    app.agents.remove(cid);
    Response::json(200, &json!({ "ok": true }))
}

fn cid_of(p: &Params<'_>) -> Result<u64, Response> {
    p.get("cid")
        .parse::<u64>()
        .map_err(|_| Response::error(400, "bad agent id"))
}

/// The run and VM index the agent sits on; 404 if unknown, 409 if it has no VM here.
pub(crate) fn place(app: &App, cid: u64) -> Result<(String, u32), Response> {
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(404, &format!("no agent {cid}")));
    };
    if let (Some(run), Some(vm)) = (agent.run, agent.vm) {
        return Ok((run, vm));
    }
    // The roster learns an agent's run from the reconcile pass, every 10 s: an agent spawned (or
    // forked, or moved) since is looked up on its node now, and the roster keeps what was found.
    match run_of(app, cid) {
        Some((run, vm)) => {
            app.agents.update(cid, |slot| {
                slot.run = Some(run.clone());
                slot.vm = Some(vm);
            });
            Ok((run, vm))
        }
        None => Err(Response::error(
            409,
            &format!("agent {cid} has no VM on this node"),
        )),
    }
}

/// The live run and VM index whose guest CID is `cid`, from its node's noded.
fn run_of(app: &App, cid: u64) -> Option<(String, u32)> {
    let Ok((200, runs)) = crate::fleet::noded_for(app, cid, "GET", "/v1/runs", None) else {
        return None;
    };
    let list = runs["runs"].as_array().or_else(|| runs.as_array())?;
    list.iter()
        .filter(|run| run["ended_at"].is_null())
        .find_map(|run| {
            let id = run["id"].as_str()?;
            run["vms"].as_array()?.iter().find_map(|vm| {
                (vm["guest_cid"].as_u64() == Some(cid))
                    .then(|| vm["id"].as_u64())
                    .flatten()
                    .and_then(|index| u32::try_from(index).ok())
                    .map(|index| (id.to_string(), index))
            })
        })
}

/// Asks noded to run `action` on the VM; any failure becomes the response.
pub(crate) fn noded_call(
    app: &App,
    cid: u64,
    run: &str,
    vm: u32,
    action: &str,
) -> Result<(), Response> {
    let path = format!("/v1/runs/{run}/vms/{vm}/{action}");
    match crate::fleet::noded_for(app, cid, "POST", &path, Some(&json!({}))) {
        Ok((code, _)) if (200..300).contains(&code) => Ok(()),
        Ok((code, body)) => Err(Response::error(
            502,
            &format!("noded {action} failed: {code} {body}"),
        )),
        Err(err) => Err(Response::error(502, &format!("noded unreachable: {err}"))),
    }
}
