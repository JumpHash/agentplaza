//! The IDE's routes for agent workspaces (docs/design/ide.md sections 6.3 to 6.5): the owner's view of
//! an agent's `/workspace`, reached through the agent's own `ide_*` tools (`crates/agent-harness/src/ide.rs`).
//!
//! Every route checks its paths with `apz_ide::path` before anything is sent, then calls the harness tool
//! through `tell` (`fleet::gateway`) and answers with the tool's JSON. The harness checks the real
//! filesystem again. The owner is the principal, so a write needs no approval. The harness takes a
//! checkpoint before each write; this module logs each owner action (a write, a restore, a run and its
//! stop) to the append-only audit (the `ide_audit` table in Postgres; `<state-dir>/ide-audit.jsonl` with no
//! database, see `db`) and publishes an `ide` bus event.
//! Owner actions do not pass the laws classifier.
//!
//! Routes (owner-only: the hub's gate in `hub_auth` covers every `/v1` path):
//!
//! | route | tool | answer |
//! |---|---|---|
//! | `GET /v1/ide/workspaces` | (roster) | `[{id, kind, owner, name, writable, status}]` |
//! | `GET /v1/ide/workspaces/{id}/tree?path=&cursor=` | `ide_list` | `{entries, next}` |
//! | `GET /v1/ide/workspaces/{id}/file?path=` | `ide_read` (chunks) | `{path, size, hash, text}` |
//! | `PUT /v1/ide/workspaces/{id}/file` `{path, text, expected_hash}` | `ide_write` | `{path, hash, checkpoint}`, or 409 `{error, current_hash}` |
//! | `GET /v1/ide/workspaces/{id}/changes?base=` | `ide_changes` (chunks) | `{base, checkpoint, diff, files, stats}` |
//! | `GET /v1/ide/workspaces/{id}/checkpoints` | `ide_checkpoints` | `{checkpoints}` |
//! | `POST /v1/ide/workspaces/{id}/checkpoints/{n}/restore` | `ide_restore` | `{restored, checkpoint}` |
//! | `GET /v1/ide/workspaces/{id}/search?q=&glob=&max=` | `ide_search` | `{matches, truncated}` |
//! | `POST /v1/ide/workspaces/{id}/runs` `{command, timeout_s?}` | `ide_run_start` | `{run}` |
//! | `GET /v1/ide/workspaces/{id}/runs/{run}?offset=` | `ide_run_poll` | `{run, offset, output, next, done, exit_code, timed_out}` |
//! | `POST /v1/ide/workspaces/{id}/runs/{run}/stop` | `ide_run_stop` | `{run, stopped, done}` |
//! | `GET /v1/ide/audit?workspace=&limit=` | (audit log) | rows, newest first |
//!
//! A tell that fails maps to: 404 unknown agent, 409 no session, 504 no answer in time, 403 a tool the
//! agent's role does not grant, 400 a refusal (`ok: false`) with the agent's text, and 502 anything else.

use apz_http::{Request, Response, Router};
use apz_ide::diff::parse_unified;
use apz_ide::path::{PathError, check_dir, check_read, check_write};
use apz_ide::workspace::WorkspaceId;
use serde_json::{Value, json};

use crate::agents::{Agent, Status};
use crate::db::Db;
use crate::plaza::percent_decode;
use crate::state::App;

/// How long a tell waits for the agent: the harness's own cap (vsockd's `MAX_TELL_TIMEOUT_MS`).
pub const TELL_TIMEOUT_MS: u64 = 5_000;
/// The most chunks one assembled read or diff takes. The harness caps a diff at 8 MiB, so this is never reached.
const MAX_CHUNKS: usize = 1_024;
const DEFAULT_AUDIT_LIMIT: usize = 100;
const MAX_AUDIT_LIMIT: usize = 1_000;

type Reply = Result<Value, Response>;

/// What the routes need from the world: the agents' tool calls, the database (the audit lives there) and the
/// bus. The live hub implements it with the fleet and the app; the tests with a fake.
pub trait Host {
    /// One tool call on agent `cid`: the gateway's status and body (vsockd's `/tell` answer).
    fn tell(&self, cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String>;
    /// The database: the IDE audit is its `ide_audit` table, or the state directory's file without one.
    fn db(&self) -> &Db;
    /// Publishes one `ide` bus event.
    fn publish(&self, data: &Value);
}

/// The live hub: tells go through the fleet, and the database and the bus are the app's.
struct Live<'a>(&'a App);

impl Host for Live<'_> {
    fn tell(&self, cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String> {
        let body = json!({ "name": name, "args": args, "timeout_ms": TELL_TIMEOUT_MS });
        crate::fleet::gateway(self.0, cid, "POST", &format!("/v1/agents/{cid}/tell"), Some(&body))
    }

    fn db(&self) -> &Db {
        &self.0.db
    }

    fn publish(&self, data: &Value) {
        self.0.bus.publish("ide", data);
    }
}

/// A tool's answer: whether it succeeded, and its text (JSON on success, a message on a refusal).
struct Answer {
    ok: bool,
    output: String,
}

/// One tool call on agent `cid`. A tell that did not answer, or a node that refused it, is the response.
fn call(host: &dyn Host, cid: u64, name: &str, args: &Value) -> Result<Answer, Response> {
    let (status, body) = host
        .tell(cid, name, args)
        .map_err(|err| Response::error(502, &format!("the agent's node did not answer: {err}")))?;
    let output = body.get("output").and_then(Value::as_str).unwrap_or_default().to_string();
    match status {
        200 => Ok(Answer { ok: body.get("ok").and_then(Value::as_bool).unwrap_or(false), output }),
        403 => Err(Response::error(403, &output)),
        404 => Err(Response::error(404, &format!("no agent {cid}"))),
        409 => Err(Response::error(409, "the agent has no session")),
        504 => Err(Response::error(504, "the agent did not answer in time (it may be busy)")),
        other => Err(Response::error(502, &format!("the agent's node answered {other}: {body}"))),
    }
}

/// The tool's JSON answer. A refusal is 400 with the agent's text.
fn reply(answer: Answer) -> Reply {
    if !answer.ok {
        return Err(Response::error(400, &answer.output));
    }
    serde_json::from_str(&answer.output)
        .map_err(|err| Response::error(502, &format!("the agent answered with text that is not JSON: {err}")))
}

/// One tool call whose answer is JSON.
fn tool(host: &dyn Host, cid: u64, name: &str, args: &Value) -> Reply {
    reply(call(host, cid, name, args)?)
}

fn respond(result: Reply) -> Response {
    match result {
        Ok(value) => Response::json(200, &value),
        Err(response) => response,
    }
}

fn workspace_of(cid: u64) -> String {
    WorkspaceId::Agent(cid).to_string()
}

/// The agent a workspace id names. Repo workspaces come in a later change.
fn workspace_cid(id: &str) -> Result<u64, Response> {
    match id.parse::<WorkspaceId>() {
        Ok(WorkspaceId::Agent(cid)) => Ok(cid),
        Ok(WorkspaceId::Repo { .. }) => Err(Response::error(501, "repo workspaces are not served yet")),
        Err(err) => Err(Response::error(400, &err.to_string())),
    }
}

/// The audit row for one owner action: when, who, where, what, plus the action's own fields.
fn audit_row(workspace: &str, kind: &str, path: Option<&str>, detail: Value) -> Value {
    let mut row = json!({
        "at_ms": apz_plaza::now_ms(),
        "actor": "owner",
        "workspace": workspace,
        "kind": kind,
        "path": path,
    });
    if let (Some(map), Value::Object(detail)) = (row.as_object_mut(), detail) {
        map.extend(detail);
    }
    row
}

/// The bus event for one owner action.
fn event(workspace: &str, path: Option<&str>, kind: &str) -> Value {
    json!({ "workspace": workspace, "path": path, "kind": kind })
}

/// Writes the audit row, then publishes the event. A row that cannot be written is a 500: the action
/// has happened, so the owner must hear that its record is missing.
fn record(host: &dyn Host, row: Value, data: Value) -> Result<(), Response> {
    host.db().append_ide_audit(&row).map_err(|err| {
        Response::error(500, &format!("the action happened but the audit log could not be written: {err}"))
    })?;
    host.publish(&data);
    Ok(())
}

/// A path, cleaned by `check`. A refused path is 400 and nothing is sent to the agent.
fn checked(raw: Option<&str>, check: fn(&str) -> Result<String, PathError>) -> Result<String, Response> {
    check(raw.unwrap_or("")).map_err(|err| Response::error(400, &err.to_string()))
}

/// A string field of a JSON body.
fn arg_str<'a>(body: &'a Value, key: &str) -> Result<&'a str, Response> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Response::error(400, &format!("\"{key}\" must be a string")))
}

fn parse_u64(raw: &str, name: &str) -> Result<u64, Response> {
    raw.parse::<u64>().map_err(|_| Response::error(400, &format!("\"{name}\" must be a whole number")))
}

/// A string field of a chunk reply.
fn text_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, Response> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Response::error(502, &format!("the agent's reply has no \"{key}\"")))
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Offline => "offline",
        Status::Idle => "idle",
        Status::Working => "working",
    }
}

/// The agent workspaces: every agent that has a VM on its node, in CID order. Repo workspaces are not
/// listed yet.
pub fn workspaces(agents: &[Agent]) -> Value {
    let mut rows: Vec<&Agent> = agents.iter().filter(|agent| agent.run.is_some() && agent.vm.is_some()).collect();
    rows.sort_by_key(|agent| agent.cid);
    Value::Array(
        rows.into_iter()
            .map(|agent| {
                json!({
                    "id": workspace_of(agent.cid),
                    "kind": "agent",
                    "owner": agent.cid,
                    "name": agent.name,
                    "writable": true,
                    "status": status_name(agent.status),
                })
            })
            .collect(),
    )
}

/// `GET .../tree`: one page of a directory. A missing `path` is the root.
pub fn tree(host: &dyn Host, cid: u64, path: Option<&str>, cursor: Option<&str>) -> Reply {
    let path = checked(path, check_dir)?;
    let mut args = json!({ "path": path });
    if let Some(cursor) = cursor {
        args["cursor"] = json!(cursor);
    }
    tool(host, cid, "ide_list", &args)
}

/// `GET .../file`: the whole text file, assembled from the harness's chunks. A file that changes while
/// it is read is 409, since the chunks would not add up to one version.
pub fn read_file(host: &dyn Host, cid: u64, path: Option<&str>) -> Reply {
    let path = checked(path, check_read)?;
    let mut text = String::new();
    let mut hash: Option<String> = None;
    let mut offset = 0usize;
    for _ in 0..MAX_CHUNKS {
        let chunk = tool(host, cid, "ide_read", &json!({ "path": path, "offset": offset }))?;
        let got = text_field(&chunk, "hash")?;
        if hash.is_none() {
            hash = Some(got.to_string());
        } else if hash.as_deref() != Some(got) {
            return Err(Response::error(409, "the file changed while it was read; read it again"));
        }
        let part = text_field(&chunk, "text")?;
        text.push_str(part);
        if chunk["done"] == json!(true) {
            return Ok(json!({ "path": path, "size": text.len(), "hash": hash, "text": text }));
        }
        if part.is_empty() {
            return Err(Response::error(502, "the agent's read made no progress"));
        }
        offset += part.len();
    }
    Err(Response::error(502, "the file took too many chunks to read"))
}

/// `PUT .../file` with `{path, text, expected_hash}`. `expected_hash` is the hash last read, or null for
/// a file that must not exist yet. A stale hash is 409 with the current one, and nothing is written. A
/// write is logged and published once it has happened.
pub fn write_file(host: &dyn Host, cid: u64, body: &Value) -> Reply {
    let path = checked(Some(arg_str(body, "path")?), check_write)?;
    let text = arg_str(body, "text")?;
    let expected = match body.get("expected_hash") {
        Some(Value::Null) => Value::Null,
        Some(Value::String(hash)) => json!(hash),
        _ => {
            return Err(Response::error(
                400,
                "\"expected_hash\" must be the hash last read, or null for a file that must not exist yet",
            ));
        }
    };
    let args = json!({ "path": path, "text": text, "expected_hash": expected });
    let answer = call(host, cid, "ide_write", &args)?;
    if !answer.ok {
        if let Some(now) = answer.output.strip_prefix("conflict: ") {
            let current = if now == "missing" { Value::Null } else { json!(now) };
            return Err(Response::json(409, &json!({ "error": "conflict", "current_hash": current })));
        }
        return Err(Response::error(400, &answer.output));
    }
    let done = reply(answer)?;
    let ws = workspace_of(cid);
    let row = audit_row(
        &ws,
        "write",
        Some(path.as_str()),
        json!({ "hash_before": expected, "hash_after": done["hash"], "checkpoint": done["checkpoint"] }),
    );
    record(host, row, event(&ws, Some(path.as_str()), "write"))?;
    Ok(done)
}

/// `GET .../changes?base=`: the diff from a base (`work_start` by default, or `checkpoint:<n>`) to the
/// workspace now, assembled from the chunks. The diff comes back parsed as well.
pub fn changes(host: &dyn Host, cid: u64, base: Option<&str>) -> Reply {
    let base = base.unwrap_or("work_start");
    let mut diff = String::new();
    let mut checkpoint = Value::Null;
    let mut offset = 0usize;
    for _ in 0..MAX_CHUNKS {
        let chunk = tool(host, cid, "ide_changes", &json!({ "base": base, "offset": offset }))?;
        if offset == 0 {
            checkpoint = chunk["checkpoint"].clone();
        }
        let part = text_field(&chunk, "diff")?;
        diff.push_str(part);
        if chunk["done"] == json!(true) {
            let set = parse_unified(&diff)
                .map_err(|err| Response::error(502, &format!("the agent's diff does not parse: {err}")))?;
            let stats = set.stats();
            return Ok(json!({
                "base": base,
                "checkpoint": checkpoint,
                "diff": diff,
                "files": set.files,
                "stats": stats,
            }));
        }
        if part.is_empty() {
            return Err(Response::error(502, "the agent's diff made no progress"));
        }
        offset += part.len();
    }
    Err(Response::error(502, "the diff took too many chunks to read"))
}

/// `GET .../checkpoints`.
pub fn checkpoints(host: &dyn Host, cid: u64) -> Reply {
    tool(host, cid, "ide_checkpoints", &json!({}))
}

/// `POST .../checkpoints/{n}/restore`. The harness takes a checkpoint of the current state first.
pub fn restore(host: &dyn Host, cid: u64, n: u64) -> Reply {
    let done = tool(host, cid, "ide_restore", &json!({ "n": n }))?;
    let ws = workspace_of(cid);
    let row = audit_row(&ws, "restore", None, json!({ "restored": n, "checkpoint": done["checkpoint"] }));
    record(host, row, event(&ws, None, "restore"))?;
    Ok(done)
}

/// `GET .../search?q=&glob=&max=`. `q` is required.
pub fn search(host: &dyn Host, cid: u64, q: Option<&str>, glob: Option<&str>, max: Option<&str>) -> Reply {
    let q = q
        .filter(|q| !q.is_empty())
        .ok_or_else(|| Response::error(400, "\"q\" is required"))?;
    let mut args = json!({ "q": q });
    if let Some(glob) = glob {
        args["glob"] = json!(glob);
    }
    if let Some(max) = max {
        args["max"] = json!(parse_u64(max, "max")?);
    }
    tool(host, cid, "ide_search", &args)
}

/// `POST .../runs` with `{command, timeout_s?}`. The run starts in the workspace and returns at once.
pub fn run_start(host: &dyn Host, cid: u64, body: &Value) -> Reply {
    let command = arg_str(body, "command")?;
    if command.trim().is_empty() {
        return Err(Response::error(400, "\"command\" is empty"));
    }
    let mut args = json!({ "command": command });
    let timeout = match body.get("timeout_s") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| Response::error(400, "\"timeout_s\" must be a whole number of seconds"))?,
        ),
    };
    if let Some(timeout) = timeout {
        args["timeout_s"] = json!(timeout);
    }
    let done = tool(host, cid, "ide_run_start", &args)?;
    let ws = workspace_of(cid);
    let row = audit_row(
        &ws,
        "run",
        None,
        json!({ "run": done["run"], "command": command, "timeout_s": timeout }),
    );
    record(host, row, event(&ws, None, "run"))?;
    Ok(done)
}

/// `GET .../runs/{run}?offset=`: the run's output from an offset, and its state.
pub fn run_poll(host: &dyn Host, cid: u64, run: &str, offset: Option<&str>) -> Reply {
    let mut args = json!({ "run": run });
    if let Some(offset) = offset {
        args["offset"] = json!(parse_u64(offset, "offset")?);
    }
    tool(host, cid, "ide_run_poll", &args)
}

/// `POST .../runs/{run}/stop`.
pub fn run_stop(host: &dyn Host, cid: u64, run: &str) -> Reply {
    let done = tool(host, cid, "ide_run_stop", &json!({ "run": run }))?;
    let ws = workspace_of(cid);
    record(host, audit_row(&ws, "run_stop", None, json!({ "run": run })), event(&ws, None, "run"))?;
    Ok(done)
}

/// `GET /v1/ide/audit?workspace=&limit=`: the newest rows first.
pub fn audit_rows(host: &dyn Host, workspace: Option<&str>, limit: Option<&str>) -> Reply {
    let limit = match limit {
        None => DEFAULT_AUDIT_LIMIT,
        Some(raw) => match parse_u64(raw, "limit")? {
            n @ 1..=1_000 => n as usize,
            _ => {
                return Err(Response::error(400, &format!("\"limit\" must be 1 to {MAX_AUDIT_LIMIT}")));
            }
        },
    };
    let rows = host
        .db()
        .ide_audit(workspace, limit as i64)
        .map_err(|err| Response::error(500, &format!("the audit log cannot be read: {err}")))?;
    Ok(Value::Array(rows))
}

/// The agent's workspace, checked to have a VM, then the core call on it.
fn with_agent(app: &App, id: &str, core: impl FnOnce(&dyn Host, u64) -> Reply) -> Reply {
    let cid = workspace_cid(id)?;
    if !app.agents.get(cid).is_some_and(|agent| agent.run.is_some() && agent.vm.is_some()) {
        return Err(Response::error(404, &format!("no workspace {}", workspace_of(cid))));
    }
    core(&Live(app), cid)
}

/// A query value, percent-decoded. An empty value is absent.
fn query(req: &Request, name: &str) -> Option<String> {
    req.query_param(name).map(percent_decode).filter(|value| !value.is_empty())
}

fn body(req: &Request) -> Result<Value, Response> {
    req.json::<Value>().map_err(|_| Response::error(400, "the body must be JSON"))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/ide/workspaces", |app, _, _| Response::json(200, &workspaces(&app.agents.list())))
        .get("/v1/ide/workspaces/{id}/tree", |app, req, p| {
            respond(with_agent(app, p.get("id"), |host, cid| {
                tree(host, cid, query(req, "path").as_deref(), query(req, "cursor").as_deref())
            }))
        })
        .get("/v1/ide/workspaces/{id}/file", |app, req, p| {
            respond(with_agent(app, p.get("id"), |host, cid| read_file(host, cid, query(req, "path").as_deref())))
        })
        .route("PUT", "/v1/ide/workspaces/{id}/file", |app, req, p| {
            respond(body(req).and_then(|body| with_agent(app, p.get("id"), |host, cid| write_file(host, cid, &body))))
        })
        .get("/v1/ide/workspaces/{id}/changes", |app, req, p| {
            respond(with_agent(app, p.get("id"), |host, cid| changes(host, cid, query(req, "base").as_deref())))
        })
        .get("/v1/ide/workspaces/{id}/checkpoints", |app, _, p| {
            respond(with_agent(app, p.get("id"), checkpoints))
        })
        .post("/v1/ide/workspaces/{id}/checkpoints/{n}/restore", |app, _, p| {
            respond(parse_u64(p.get("n"), "n").and_then(|n| with_agent(app, p.get("id"), |host, cid| restore(host, cid, n))))
        })
        .get("/v1/ide/workspaces/{id}/search", |app, req, p| {
            respond(with_agent(app, p.get("id"), |host, cid| {
                search(
                    host,
                    cid,
                    query(req, "q").as_deref(),
                    query(req, "glob").as_deref(),
                    query(req, "max").as_deref(),
                )
            }))
        })
        .post("/v1/ide/workspaces/{id}/runs", |app, req, p| {
            respond(body(req).and_then(|body| with_agent(app, p.get("id"), |host, cid| run_start(host, cid, &body))))
        })
        .get("/v1/ide/workspaces/{id}/runs/{run}", |app, req, p| {
            respond(with_agent(app, p.get("id"), |host, cid| {
                run_poll(host, cid, p.get("run"), query(req, "offset").as_deref())
            }))
        })
        .post("/v1/ide/workspaces/{id}/runs/{run}/stop", |app, _, p| {
            respond(with_agent(app, p.get("id"), |host, cid| run_stop(host, cid, p.get("run"))))
        })
        .get("/v1/ide/audit", |app, req, _| {
            respond(audit_rows(&Live(app), query(req, "workspace").as_deref(), query(req, "limit").as_deref()))
        })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};

    use apz_proto::chat::Usage;

    use super::*;

    type Answerer = Box<dyn Fn(&str, &Value) -> Result<(u16, Value), String>>;

    /// A host whose tool calls `answer` decides. It records each call and each event.
    struct Fake {
        answer: Answerer,
        log: RefCell<Vec<(String, Value)>>,
        events: RefCell<Vec<Value>>,
        /// No database: the audit is the state directory's file (`dir`).
        db: Db,
        dir: std::path::PathBuf,
    }

    impl Fake {
        fn new(tag: &str, answer: impl Fn(&str, &Value) -> Result<(u16, Value), String> + 'static) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
            let dir = std::env::temp_dir().join(format!("apid-ide-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                answer: Box::new(answer),
                log: RefCell::default(),
                events: RefCell::default(),
                db: Db::files_only(&dir),
                dir,
            }
        }

        /// The tool calls made so far.
        fn calls(&self) -> usize {
            self.log.borrow().len()
        }

        fn last_args(&self) -> Value {
            self.log.borrow().last().map(|(_, args)| args.clone()).unwrap_or(Value::Null)
        }
    }

    impl Host for Fake {
        fn tell(&self, _cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String> {
            self.log.borrow_mut().push((name.to_string(), args.clone()));
            (self.answer)(name, args)
        }

        fn db(&self) -> &Db {
            &self.db
        }

        fn publish(&self, data: &Value) {
            self.events.borrow_mut().push(data.clone());
        }
    }

    /// A harness answer that succeeded, with `output` as its JSON text.
    fn ok(output: Value) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": true, "output": output.to_string() })))
    }

    /// A harness answer that refused, with `text` as its message.
    fn refused(text: &str) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": false, "output": text })))
    }

    fn status_of<T>(result: Result<T, Response>) -> u16 {
        match result {
            Ok(_) => 200,
            Err(response) => response.status,
        }
    }

    fn body_of(response: Response) -> Value {
        serde_json::from_slice(response.bytes()).unwrap()
    }

    fn agent(cid: u64, run: Option<&str>, vm: Option<u32>, status: Status) -> Agent {
        Agent {
            cid,
            name: format!("agent-{cid}"),
            role: "worker".into(),
            model_override: None,
            status,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: run.map(str::to_string),
            vm,
            usage: Usage::default(),
            species: String::new(),
            kind: "llm".into(),
        }
    }

    #[test]
    fn workspace_ids_parse_and_repo_ones_wait() {
        assert_eq!(workspace_cid("agent:12").ok(), Some(12));
        assert_eq!(status_of(workspace_cid("agent:012")), 400);
        assert_eq!(status_of(workspace_cid("repo:tool@main")), 501);
        assert_eq!(status_of(workspace_cid("nope")), 400);
        assert_eq!(workspace_of(12), "agent:12");
    }

    #[test]
    fn workspaces_list_only_agents_with_a_vm_in_cid_order() {
        let agents = vec![
            agent(42, Some("run-a"), Some(3), Status::Working),
            agent(7, None, None, Status::Offline),
            agent(9, Some("run-a"), Some(1), Status::Idle),
        ];
        let rows = workspaces(&agents);
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], json!("agent:9"));
        assert_eq!(rows[0]["status"], json!("idle"));
        assert_eq!(rows[1]["id"], json!("agent:42"));
        assert_eq!(rows[1]["status"], json!("working"));
        assert_eq!(rows[1]["owner"], json!(42));
        assert_eq!(rows[1]["kind"], json!("agent"));
        assert_eq!(rows[1]["writable"], json!(true));
    }

    #[test]
    fn tree_cleans_the_path_and_forwards_the_cursor() {
        let fake = Fake::new("tree", |_, _| ok(json!({ "entries": [], "next": null })));
        assert!(tree(&fake, 12, Some("docs/"), Some("0a")).is_ok());
        assert_eq!(fake.last_args(), json!({ "path": "docs", "cursor": "0a" }));
        assert!(tree(&fake, 12, None, None).is_ok());
        assert_eq!(fake.last_args(), json!({ "path": "" }));
    }

    #[test]
    fn read_assembles_chunks() {
        const TEXT: &str = "the quick brown fox jumps over the lazy dog";
        let fake = Fake::new("read", |name, args| {
            assert_eq!(name, "ide_read");
            let offset = args["offset"].as_u64().unwrap() as usize;
            let end = (offset + 7).min(TEXT.len());
            ok(json!({
                "path": "a.txt",
                "size": TEXT.len(),
                "hash": "h1",
                "offset": offset,
                "text": &TEXT[offset..end],
                "done": end == TEXT.len(),
            }))
        });
        let file = read_file(&fake, 12, Some("a.txt")).unwrap();
        assert_eq!(file["text"], json!(TEXT));
        assert_eq!(file["hash"], json!("h1"));
        assert_eq!(file["size"], json!(TEXT.len()));
        assert_eq!(fake.calls(), TEXT.len().div_ceil(7));
    }

    #[test]
    fn read_refuses_a_file_that_changes_while_it_is_read() {
        let fake = Fake::new("moving", |_, args| {
            let offset = args["offset"].as_u64().unwrap();
            let (hash, text, done) = if offset == 0 { ("h1", "abcd", false) } else { ("h2", "efgh", true) };
            ok(json!({ "path": "a.txt", "size": 8, "hash": hash, "offset": offset, "text": text, "done": done }))
        });
        assert_eq!(status_of(read_file(&fake, 12, Some("a.txt"))), 409);
    }

    #[test]
    fn bad_paths_are_refused_before_any_tell() {
        let fake = Fake::new("refuse", |_, _| ok(json!({})));
        for raw in ["", "../x", "/etc/passwd", ".env", "keys/id_rsa.pem"] {
            assert_eq!(status_of(read_file(&fake, 12, Some(raw))), 400, "read {raw:?}");
        }
        assert_eq!(status_of(tree(&fake, 12, Some("a/../b"), None)), 400);
        assert_eq!(status_of(tree(&fake, 12, Some(".ssh"), None)), 400);
        let write = json!({ "path": ".plaza/state.json", "text": "x", "expected_hash": null });
        assert_eq!(status_of(write_file(&fake, 12, &write)), 400);
        let secret = json!({ "path": "config/.env", "text": "x", "expected_hash": null });
        assert_eq!(status_of(write_file(&fake, 12, &secret)), 400);
        assert_eq!(fake.calls(), 0);
    }

    #[test]
    fn a_stale_write_is_a_conflict_and_is_not_logged() {
        let fake = Fake::new("conflict", |_, args| {
            if args["expected_hash"] == json!("stale") {
                refused("conflict: abc123")
            } else {
                ok(json!({ "path": "a.txt", "hash": "new", "checkpoint": 7 }))
            }
        });
        let stale = json!({ "path": "a.txt", "text": "two", "expected_hash": "stale" });
        let response = write_file(&fake, 12, &stale).unwrap_err();
        assert_eq!(response.status, 409);
        let body = body_of(response);
        assert_eq!(body["error"], json!("conflict"));
        assert_eq!(body["current_hash"], json!("abc123"));
        assert!(fake.db.ide_audit(None, 10).unwrap().is_empty());
        assert!(fake.events.borrow().is_empty());
    }

    #[test]
    fn a_write_to_a_missing_file_says_so() {
        let fake = Fake::new("missing", |_, _| refused("conflict: missing"));
        let body = json!({ "path": "gone.txt", "text": "x", "expected_hash": "h0" });
        let response = write_file(&fake, 12, &body).unwrap_err();
        assert_eq!(response.status, 409);
        assert_eq!(body_of(response)["current_hash"], Value::Null);
    }

    #[test]
    fn a_write_is_audited_and_published_and_survives_a_reopen() {
        let fake = Fake::new("write", |_, _| ok(json!({ "path": "docs/a.txt", "hash": "new", "checkpoint": 7 })));
        let body = json!({ "path": "docs/a.txt", "text": "two", "expected_hash": "old" });
        let done = write_file(&fake, 12, &body).unwrap();
        assert_eq!(done["hash"], json!("new"));

        let rows = fake.db.ide_audit(Some("agent:12"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kind"], json!("write"));
        assert_eq!(rows[0]["actor"], json!("owner"));
        assert_eq!(rows[0]["path"], json!("docs/a.txt"));
        assert_eq!(rows[0]["hash_before"], json!("old"));
        assert_eq!(rows[0]["hash_after"], json!("new"));
        assert_eq!(rows[0]["checkpoint"], json!(7));
        assert!(rows[0]["at_ms"].as_u64().is_some());
        assert_eq!(fake.events.borrow()[0], json!({ "workspace": "agent:12", "path": "docs/a.txt", "kind": "write" }));

        // The row is on disk: a fresh handle on the same file reads it back.
        let reopened = Db::files_only(&fake.dir);
        assert_eq!(reopened.ide_audit(None, 10).unwrap().len(), 1);
    }

    #[test]
    fn changes_assemble_chunks_and_parse_the_diff() {
        const DIFF: &str = "diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
diff --git a/b.txt b/b.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/b.txt
@@ -0,0 +1 @@
+hello
";
        let fake = Fake::new("changes", |name, args| {
            assert_eq!(name, "ide_changes");
            let offset = args["offset"].as_u64().unwrap() as usize;
            let end = (offset + 40).min(DIFF.len());
            ok(json!({
                "base": "work_start",
                "checkpoint": 5,
                "offset": offset,
                "total": DIFF.len(),
                "diff": &DIFF[offset..end],
                "done": end == DIFF.len(),
            }))
        });
        let reply = changes(&fake, 12, None).unwrap();
        assert_eq!(reply["base"], json!("work_start"));
        assert_eq!(reply["checkpoint"], json!(5));
        assert_eq!(reply["diff"], json!(DIFF));
        assert_eq!(reply["files"].as_array().unwrap().len(), 2);
        assert_eq!(reply["files"][1]["status"], json!("added"));
        assert_eq!(reply["stats"]["files"], json!(2));
        assert_eq!(reply["stats"]["additions"], json!(2));
        assert_eq!(reply["stats"]["deletions"], json!(1));
    }

    #[test]
    fn an_empty_diff_has_no_files() {
        let fake = Fake::new("empty", |_, _| ok(json!({ "base": "work_start", "checkpoint": 1, "offset": 0, "total": 0, "diff": "", "done": true })));
        let reply = changes(&fake, 12, None).unwrap();
        assert_eq!(reply["files"], json!([]));
        assert_eq!(reply["stats"]["files"], json!(0));
    }

    #[test]
    fn changes_forward_the_base() {
        let fake = Fake::new("base", |_, _| ok(json!({ "base": "checkpoint:4", "checkpoint": 4, "offset": 0, "total": 0, "diff": "", "done": true })));
        assert!(changes(&fake, 12, Some("checkpoint:4")).is_ok());
        assert_eq!(fake.last_args(), json!({ "base": "checkpoint:4", "offset": 0 }));
    }

    #[test]
    fn restore_is_audited() {
        let fake = Fake::new("restore", |_, _| ok(json!({ "restored": 3, "checkpoint": 9 })));
        let done = restore(&fake, 12, 3).unwrap();
        assert_eq!(done["checkpoint"], json!(9));
        let rows = fake.db.ide_audit(Some("agent:12"), 10).unwrap();
        assert_eq!(rows[0]["kind"], json!("restore"));
        assert_eq!(rows[0]["restored"], json!(3));
        assert_eq!(fake.events.borrow()[0]["kind"], json!("restore"));
    }

    #[test]
    fn runs_are_audited_newest_first() {
        let fake = Fake::new("runs", |name, _| match name {
            "ide_run_start" => ok(json!({ "run": "run-1" })),
            _ => ok(json!({ "run": "run-1", "stopped": true, "done": true })),
        });
        let started = run_start(&fake, 12, &json!({ "command": "cargo test", "timeout_s": 30 })).unwrap();
        assert_eq!(started["run"], json!("run-1"));
        assert_eq!(fake.last_args(), json!({ "command": "cargo test", "timeout_s": 30 }));
        run_stop(&fake, 12, "run-1").unwrap();

        let rows = audit_rows(&fake, Some("agent:12"), None).unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], json!("run_stop"));
        assert_eq!(rows[1]["kind"], json!("run"));
        assert_eq!(rows[1]["command"], json!("cargo test"));
        assert_eq!(rows[1]["timeout_s"], json!(30));
        assert_eq!(fake.events.borrow().len(), 2);
    }

    #[test]
    fn a_run_with_a_bad_command_is_refused_before_any_tell() {
        let fake = Fake::new("badrun", |_, _| ok(json!({})));
        assert_eq!(status_of(run_start(&fake, 12, &json!({ "command": "   " }))), 400);
        assert_eq!(status_of(run_start(&fake, 12, &json!({ "command": "ls", "timeout_s": "soon" }))), 400);
        assert_eq!(status_of(run_start(&fake, 12, &json!({}))), 400);
        assert_eq!(fake.calls(), 0);
        assert!(fake.db.ide_audit(None, 10).unwrap().is_empty());
    }

    #[test]
    fn search_needs_a_query_and_forwards_the_rest() {
        let fake = Fake::new("search", |_, _| ok(json!({ "matches": [], "truncated": false })));
        assert_eq!(status_of(search(&fake, 12, None, None, None)), 400);
        assert_eq!(status_of(search(&fake, 12, Some(""), None, None)), 400);
        assert_eq!(fake.calls(), 0);
        assert!(search(&fake, 12, Some("needle"), Some("*.rs"), Some("5")).is_ok());
        assert_eq!(fake.last_args(), json!({ "q": "needle", "glob": "*.rs", "max": 5 }));
        assert_eq!(status_of(search(&fake, 12, Some("needle"), None, Some("many"))), 400);
    }

    #[test]
    fn a_failed_tell_maps_to_its_status() {
        type Tell = fn(&str, &Value) -> Result<(u16, Value), String>;
        let cases: [(&str, Tell, u16); 7] = [
            ("timeout", |_, _| Ok((504, json!({ "error": "x" }))), 504),
            ("no session", |_, _| Ok((409, json!({ "error": "x" }))), 409),
            ("unknown cid", |_, _| Ok((404, json!({ "error": "x" }))), 404),
            ("role", |_, _| Ok((403, json!({ "ok": false, "output": "role does not grant ide_list" }))), 403),
            ("node down", |_, _| Err("node n2 is offline".to_string()), 502),
            ("refusal", |_, _| refused("denied: secret files are not available"), 400),
            ("not json", |_, _| Ok((200, json!({ "ok": true, "output": "nope" }))), 502),
        ];
        for (tag, answer, want) in cases {
            let fake = Fake::new(tag, answer);
            assert_eq!(status_of(tree(&fake, 12, None, None)), want, "{tag}");
        }
    }

    #[test]
    fn an_unknown_tell_text_is_the_refusal_text() {
        let fake = Fake::new("text", |_, _| refused("denied: secret files are not available"));
        let response = tree(&fake, 12, None, None).unwrap_err();
        assert_eq!(body_of(response)["error"], json!("denied: secret files are not available"));
    }

    #[test]
    fn the_audit_log_filters_and_checks_its_limit() {
        let fake = Fake::new("auditq", |_, _| ok(json!({ "run": "run-1" })));
        fake.db.append_ide_audit(&json!({ "at_ms": 1, "workspace": "agent:12", "kind": "write", "n": 1 })).unwrap();
        fake.db.append_ide_audit(&json!({ "at_ms": 2, "workspace": "agent:40", "kind": "write", "n": 2 })).unwrap();
        fake.db.append_ide_audit(&json!({ "at_ms": 3, "workspace": "agent:12", "kind": "restore", "n": 3 })).unwrap();

        let mine = audit_rows(&fake, Some("agent:12"), None).unwrap();
        assert_eq!(mine.as_array().unwrap().len(), 2);
        assert_eq!(mine[0]["n"], json!(3));

        let newest = audit_rows(&fake, None, Some("1")).unwrap();
        assert_eq!(newest.as_array().unwrap().len(), 1);
        assert_eq!(newest[0]["n"], json!(3));

        assert_eq!(status_of(audit_rows(&fake, None, Some("0"))), 400);
        assert_eq!(status_of(audit_rows(&fake, None, Some("1001"))), 400);
        assert_eq!(status_of(audit_rows(&fake, None, Some("abc"))), 400);
    }

    #[test]
    fn a_checkpoint_number_must_be_a_number() {
        let fake = Fake::new("cp", |_, _| ok(json!({ "checkpoints": [] })));
        assert!(checkpoints(&fake, 12).is_ok());
        assert_eq!(status_of(parse_u64("3x", "n")), 400);
        assert_eq!(parse_u64("3", "n").ok(), Some(3));
    }
}
