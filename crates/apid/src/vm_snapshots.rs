//! Live checkpoints, rollback and forks of an agent's whole VM — memory, disk and running processes,
//! not only its files (those are the IDE's file checkpoints, `/v1/ide/workspaces/{id}/checkpoints`).
//!
//! GET    /v1/agents/{cid}/vm-checkpoints                 → {checkpoints: [checkpoint]} (newest first)
//! POST   /v1/agents/{cid}/vm-checkpoints  {label?}       → checkpoint
//! POST   /v1/agents/{cid}/vm-checkpoints/{name}/restore  → {ok, safety: checkpoint}
//! DELETE /v1/agents/{cid}/vm-checkpoints/{name}          → {ok}
//! POST   /v1/agents/{cid}/fork            {n}            → {forks: [{cid, run, name}], checkpoint}
//!
//! A checkpoint is noded's snapshot view (`noded::snapshots::SnapshotView`). Rolling back first takes
//! a safety checkpoint (`before-restore`), so a rollback can itself be undone — CubeSandbox's rollback
//! deletes the VM before it restores, and a failed restore loses it. Forks are new agents of the same
//! role, each with a copy of the source agent's VM at the fork's checkpoint and a name of its own.
//!
//! The laws decide when a checkpoint is taken on their own: before a tool call whose risk class is
//! high, the hub asks for one (see [`auto_checkpoint`]), so "undo the last risky step" restores the
//! whole machine.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::fleet::{LOCAL, node_of};
use crate::lifecycle::place;
use crate::state::App;

/// Longest checkpoint label, in characters.
pub const LABEL_MAX: usize = 40;
/// The label of a checkpoint the owner or the console takes without naming it.
const DEFAULT_LABEL: &str = "manual";
/// Most forks one request makes.
const FORK_MAX: u32 = 8;
/// At most one automatic checkpoint per agent in this window (milliseconds).
const AUTO_WINDOW_MS: u64 = 60_000;
/// How long an automatic checkpoint may hold up a tool call before it is abandoned.
const AUTO_TIMEOUT: Duration = Duration::from_secs(5);
/// A checkpoint and a fork copy guest memory, so they get more time than a plain noded call.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(60);

/// When each agent last had a checkpoint taken, in ms since the epoch (manual or automatic).
static LAST_CHECKPOINT: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());

#[derive(Deserialize, Default)]
struct CreateBody {
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct ForkBody {
    n: u32,
}

#[derive(Deserialize)]
struct ForkedVm {
    cid: u64,
    run: String,
}

#[derive(Deserialize)]
struct ForkReply {
    forks: Vec<ForkedVm>,
    checkpoint: String,
}

/// Routes listed in the module docs.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agents/{cid}/vm-checkpoints", list_route)
        .doc(RouteDoc::new("List an agent's VM checkpoints").errors(&[502]))
        .post("/v1/agents/{cid}/vm-checkpoints", create_route)
        .doc(RouteDoc::new("Take a checkpoint of an agent's VM").errors(&[400, 502]))
        .post(
            "/v1/agents/{cid}/vm-checkpoints/{name}/restore",
            restore_route,
        )
        .doc(RouteDoc::new("Roll an agent's VM back to a checkpoint").errors(&[400, 502]))
        .delete("/v1/agents/{cid}/vm-checkpoints/{name}", delete_route)
        .doc(RouteDoc::new("Delete a VM checkpoint").errors(&[400, 502]))
        .post("/v1/agents/{cid}/fork", fork_route)
        .doc(RouteDoc::new("Fork an agent's VM into new agents").errors(&[400, 404, 502]))
}

/// Takes a checkpoint of `cid`'s VM before the risky tool call `tool` runs, when automatic
/// checkpoints are on and none was taken for this agent in the last minute. Best effort: a failure
/// is logged and the call goes ahead.
pub fn auto_checkpoint(app: &App, cid: u64, tool: &str) {
    if !app.settings.get().vm_auto_checkpoints || !is_high_risk(tool) {
        return;
    }
    if !claim_window(cid, apz_plaza::now_ms()) {
        return;
    }
    let Ok((run, vm)) = place(app, cid) else {
        return;
    };
    let label = auto_label(tool);
    let path = checkpoints_path(&run, vm);
    let body = json!({ "label": label });
    match noded_call_value(app, cid, "POST", &path, Some(&body), AUTO_TIMEOUT) {
        Ok((code, view)) if (200..300).contains(&code) => {
            app.transcripts.record(
                &app.bus,
                cid,
                0,
                "vm_checkpoint",
                json!({ "checkpoint": view, "auto": true }),
            );
            publish_agent(app, cid);
        }
        Ok((code, body)) => {
            eprintln!("apid: automatic checkpoint of agent {cid} refused: {code} {body}");
        }
        Err(err) => eprintln!("apid: automatic checkpoint of agent {cid} failed: {err}"),
    }
}

/// Whether a tool's risk class is `irreversible` (the laws' C-004a class: destructive or
/// irreversible actions, such as a deleted repository or a created cloud resource). Reads and
/// ordinary writes are not checkpointed; the laws' `write` is a reviewable change, not a point of
/// no return.
pub(crate) fn is_high_risk(tool: &str) -> bool {
    crate::laws::risk_class(tool) == "irreversible"
}

/// Records a checkpoint of `cid` at `now` and says whether one may be taken: false when the
/// previous one (manual or automatic) is less than a minute old.
fn claim_window(cid: u64, now: u64) -> bool {
    let mut last = LAST_CHECKPOINT
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let recent = last
        .get(&cid)
        .is_some_and(|&at| now.saturating_sub(at) < AUTO_WINDOW_MS);
    if !recent {
        last.insert(cid, now);
    }
    !recent
}

/// Notes a checkpoint of `cid` taken now, so the automatic ones wait a minute.
fn note_checkpoint(cid: u64) {
    LAST_CHECKPOINT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(cid, apz_plaza::now_ms());
}

/// `before-<tool>` as a label: the tool name slugged, cut to [`LABEL_MAX`].
pub(crate) fn auto_label(tool: &str) -> String {
    let slug = slug(tool);
    let label = if slug.is_empty() {
        "before-tool".to_owned()
    } else {
        format!("before-{slug}")
    };
    label
        .chars()
        .take(LABEL_MAX)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

/// Lower-case ASCII letters and digits, every other run of characters as one `-`, no edge dashes.
pub(crate) fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// A checkpoint label the owner gave: 1 to [`LABEL_MAX`] characters, no control characters.
pub(crate) fn check_label(label: &str) -> Result<String, String> {
    let len = label.chars().count();
    if !(1..=LABEL_MAX).contains(&len) {
        return Err(format!("label must be 1 to {LABEL_MAX} characters"));
    }
    if label.chars().any(char::is_control) {
        return Err("label must not contain control characters".into());
    }
    Ok(label.to_owned())
}

/// A checkpoint name as noded issues them (`cp-…`): ASCII letters, digits, `-` and `_`. The name
/// goes into a path, so anything else is refused before noded sees it.
pub(crate) fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err("bad checkpoint name".into())
    }
}

/// How many forks a request asks for: 1 to [`FORK_MAX`].
pub(crate) fn check_fork_count(n: u32) -> Result<u32, String> {
    if (1..=FORK_MAX).contains(&n) {
        Ok(n)
    } else {
        Err(format!("n must be 1 to {FORK_MAX}"))
    }
}

/// The names of `count` forks of an agent called `base`: `<base>-fork-1`, `<base>-fork-2`, … skipping
/// any name `taken` reports, so a second fork round does not reuse a name.
pub(crate) fn fork_names(base: &str, count: usize, taken: impl Fn(&str) -> bool) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(count);
    let mut k = 1u32;
    while names.len() < count {
        let candidate = format!("{base}-fork-{k}");
        if !names.contains(&candidate) && !taken(&candidate) {
            names.push(candidate);
        }
        k += 1;
    }
    names
}

fn list_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    let path = checkpoints_path(&run, vm);
    match relay(noded_call_value(
        app,
        cid,
        "GET",
        &path,
        None,
        SNAPSHOT_TIMEOUT,
    )) {
        Ok(list) => Response::json(200, &list),
        Err(res) => res,
    }
}

fn create_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let label = match label_of(req) {
        Ok(label) => label,
        Err(res) => return res,
    };
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    let path = checkpoints_path(&run, vm);
    let body = json!({ "label": label });
    let view = match relay(noded_call_value(
        app,
        cid,
        "POST",
        &path,
        Some(&body),
        SNAPSHOT_TIMEOUT,
    )) {
        Ok(view) => view,
        Err(res) => return res,
    };
    note_checkpoint(cid);
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "vm_checkpoint",
        json!({ "checkpoint": view, "auto": false }),
    );
    publish_agent(app, cid);
    Response::json(200, &view)
}

fn restore_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let name = p.get("name");
    if let Err(err) = check_name(name) {
        return Response::error(400, &err);
    }
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    // The agent's work stops first: a restore under a running model call would change its VM
    // under a turn that is still reading the old machine.
    app.work
        .cancel(app, cid, &format!("rolled back to checkpoint {name}"));
    let path = format!("{}/{name}/restore", checkpoints_path(&run, vm));
    let answer = match relay(noded_call_value(
        app,
        cid,
        "POST",
        &path,
        Some(&json!({})),
        SNAPSHOT_TIMEOUT,
    )) {
        Ok(answer) => answer,
        Err(res) => return res,
    };
    let safety = answer.get("snapshot").cloned().unwrap_or(Value::Null);
    note_checkpoint(cid);
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "vm_restore",
        json!({ "name": name, "safety": safety }),
    );
    publish_agent(app, cid);
    Response::json(200, &json!({ "ok": true, "safety": safety }))
}

fn delete_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let name = p.get("name");
    if let Err(err) = check_name(name) {
        return Response::error(400, &err);
    }
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    let path = format!("{}/{name}", checkpoints_path(&run, vm));
    if let Err(res) = relay(noded_call_value(
        app,
        cid,
        "DELETE",
        &path,
        None,
        SNAPSHOT_TIMEOUT,
    )) {
        return res;
    }
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "vm_checkpoint",
        json!({ "deleted": name }),
    );
    Response::json(200, &json!({ "ok": true }))
}

fn fork_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let cid = match cid_of(p) {
        Ok(cid) => cid,
        Err(res) => return res,
    };
    let n = match serde_json::from_slice::<ForkBody>(&req.body)
        .map_err(|err| format!("bad body: {err}"))
        .and_then(|body| check_fork_count(body.n))
    {
        Ok(n) => n,
        Err(err) => return Response::error(400, &err),
    };
    let Some(agent) = app.agents.get(cid) else {
        return Response::error(404, &format!("no agent {cid}"));
    };
    let (run, vm) = match place(app, cid) {
        Ok(place) => place,
        Err(res) => return res,
    };
    let path = format!("/v1/runs/{run}/vms/{vm}/fork");
    let answer = match relay(noded_call_value(
        app,
        cid,
        "POST",
        &path,
        Some(&json!({ "n": n })),
        SNAPSHOT_TIMEOUT,
    )) {
        Ok(answer) => answer,
        Err(res) => return res,
    };
    let reply: ForkReply = match serde_json::from_value(answer) {
        Ok(reply) => reply,
        Err(err) => return Response::error(502, &format!("noded fork answer: {err}")),
    };
    let base = if agent.name.is_empty() {
        format!("agent-{cid}")
    } else {
        agent.name.clone()
    };
    let names = fork_names(&base, reply.forks.len(), |candidate| {
        app.names.used_by_other(u64::MAX, candidate)
    });
    let mut forks = Vec::with_capacity(reply.forks.len());
    for (fork, name) in reply.forks.iter().zip(names) {
        // The fork's first hello is matched to this role and model (see `Spawned::expect`), and it works on
        // its parent's project.
        app.spawned.expect(
            &agent.role,
            agent.model_override.clone(),
            app.members.project_of(cid),
        );
        if let Err(err) = app.names.set(fork.cid, &name) {
            eprintln!("apid: name of fork {} not saved: {err}", fork.cid);
        }
        app.agents.update(fork.cid, |a| a.name.clone_from(&name));
        app.transcripts.record(
            &app.bus,
            fork.cid,
            0,
            "forked_from",
            json!({ "cid": cid, "checkpoint": reply.checkpoint }),
        );
        forks.push(json!({ "cid": fork.cid, "run": fork.run, "name": name }));
    }
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        "fork",
        json!({ "forks": forks, "checkpoint": reply.checkpoint }),
    );
    publish_agent(app, cid);
    Response::json(
        200,
        &json!({ "forks": forks, "checkpoint": reply.checkpoint }),
    )
}

/// A noded call for the agent's VM with its own timeout. The local node is called directly, so a
/// slow snapshot does not hold the default timeout's connection; a remote node goes through the
/// fleet queue, which has its own deadline. `(status, json)`, or the reason noded was unreachable.
fn noded_call_value(
    app: &App,
    cid: u64,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    if node_of(app, cid) != LOCAL {
        return crate::fleet::noded_for(app, cid, method, path, body);
    }
    let bytes = body.map(Value::to_string);
    let reply = app
        .noded
        .send(method, path, bytes.as_deref().map(str::as_bytes), timeout)
        .map_err(|e| format!("127.0.0.1:{} unreachable: {e}", app.noded.port))?;
    let json = if reply.body.is_empty() {
        Value::Null
    } else {
        reply.json().unwrap_or(Value::Null)
    };
    Ok((reply.status, json))
}

/// The checkpoints collection of the agent's VM on its run.
fn checkpoints_path(run: &str, vm: u32) -> String {
    format!("/v1/runs/{run}/vms/{vm}/checkpoints")
}

/// The label in the body: absent or empty means [`DEFAULT_LABEL`]; a body that is not a JSON object
/// with a string `label` is refused.
fn label_of(req: &Request) -> Result<String, Response> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(DEFAULT_LABEL.to_owned());
    }
    let body: CreateBody = serde_json::from_slice(&req.body)
        .map_err(|err| Response::error(400, &format!("bad body: {err}")))?;
    match body.label {
        None => Ok(DEFAULT_LABEL.to_owned()),
        Some(label) => check_label(&label).map_err(|err| Response::error(400, &err)),
    }
}

/// Noded's answer as the console's: its body on success, its status on a client error, 502 otherwise.
fn relay(result: Result<(u16, Value), String>) -> Result<Value, Response> {
    match result {
        Ok((code, body)) if (200..300).contains(&code) => Ok(body),
        Ok((code, body)) if (400..500).contains(&code) => Err(Response::json(code, &body)),
        Ok((code, body)) => Err(Response::error(502, &format!("noded: {code} {body}"))),
        Err(err) => Err(Response::error(502, &format!("noded unreachable: {err}"))),
    }
}

fn publish_agent(app: &App, cid: u64) {
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
}

fn cid_of(p: &Params<'_>) -> Result<u64, Response> {
    p.get("cid")
        .parse::<u64>()
        .map_err(|_| Response::error(400, "bad agent id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(body: &str) -> Request {
        Request {
            method: "POST".into(),
            path: "/v1/agents/1/vm-checkpoints".into(),
            query: String::new(),
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
            peer: None,
        }
    }

    #[test]
    fn labels_are_one_to_forty_characters_without_controls() {
        assert!(check_label("").is_err());
        assert_eq!(check_label("before lunch").unwrap(), "before lunch");
        assert!(check_label(&"x".repeat(LABEL_MAX)).is_ok());
        assert!(check_label(&"x".repeat(LABEL_MAX + 1)).is_err());
        // Characters, not bytes: forty accented letters are forty characters.
        assert!(check_label(&"é".repeat(LABEL_MAX)).is_ok());
        assert!(check_label("tab\there").is_err());
    }

    #[test]
    fn a_missing_or_empty_body_is_the_manual_label() {
        assert_eq!(label_of(&request("")).unwrap(), DEFAULT_LABEL);
        assert_eq!(label_of(&request("  \n")).unwrap(), DEFAULT_LABEL);
        assert_eq!(label_of(&request("{}")).unwrap(), DEFAULT_LABEL);
        assert_eq!(
            label_of(&request(r#"{"label":"step 3"}"#)).unwrap(),
            "step 3"
        );
        assert!(label_of(&request(r#"{"label":""}"#)).is_err());
        assert!(label_of(&request("not json")).is_err());
    }

    #[test]
    fn checkpoint_names_are_path_safe() {
        assert!(check_name("cp-1a2b_3").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("../etc").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name(&"c".repeat(65)).is_err());
    }

    #[test]
    fn a_fork_makes_one_to_eight() {
        assert!(check_fork_count(0).is_err());
        assert_eq!(check_fork_count(1).unwrap(), 1);
        assert_eq!(check_fork_count(8).unwrap(), 8);
        assert!(check_fork_count(9).is_err());
    }

    #[test]
    fn fork_names_count_up_and_skip_taken_names() {
        assert_eq!(
            fork_names("Ada", 2, |_| false),
            vec!["Ada-fork-1", "Ada-fork-2"]
        );
        assert_eq!(
            fork_names("Ada", 2, |name| name == "Ada-fork-1"),
            vec!["Ada-fork-2", "Ada-fork-3"]
        );
        assert!(fork_names("Ada", 0, |_| false).is_empty());
    }

    #[test]
    fn automatic_checkpoints_wait_a_minute_per_agent() {
        // Each test uses its own CIDs: the window is process-wide.
        let cid = 9_100_001;
        let start = 1_000_000;
        assert!(
            claim_window(cid, start),
            "the first checkpoint may be taken"
        );
        assert!(
            !claim_window(cid, start + 59_999),
            "a second one inside the minute waits"
        );
        assert!(
            claim_window(cid, start + 60_000 + 59_999 + 1),
            "after a minute, the next may"
        );
        assert!(
            claim_window(9_100_002, start),
            "another agent has its own window"
        );
    }

    #[test]
    fn only_irreversible_tools_are_checkpointed() {
        assert!(!is_high_risk("read"));
        assert!(!is_high_risk("todo_write"));
        // An unknown tool is a plain write, which is reviewable and not checkpointed.
        assert!(!is_high_risk("some_unknown_tool"));
    }
}
