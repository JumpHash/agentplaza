//! The IDE's routes (docs/design/ide.md sections 6.3 to 6.5). A workspace is one of three kinds:
//!
//! - `agent:<cid>`: the owner's view of an agent's `/workspace`, reached through the agent's own `ide_*` tools
//!   (`crates/agent-harness/src/ide.rs`). The harness takes the checkpoints and checks the real filesystem.
//! - `repo:<name>@<ref>`: a project's repo in Weft, on `main`. Reads come from main's head, and an edit is the
//!   owner's edit, which lands at once (signed for the owner by the hub). A checkpoint is a landing. Only project
//!   repos are offered; the platform's own source repo never is (`PLATFORM_REPOS`).
//! - `station:<id>`: an online workstation's work folder (docs/design/workstations.md section 3). Its files, runs
//!   and terminals go to the workstation's daemon over its live link, which scopes them to its work folder. A
//!   workstation has no checkpoints, so it has no changes, no restore and no branch. STOP ALL refuses every
//!   operation on it.
//!
//! Every route checks its paths with `apz_ide::path` before anything is sent. This module logs each owner
//! action (a write, a restore, a run and its stop) to the append-only audit (the
//! `ide_audit` table in Postgres; `<state-dir>/ide-audit.jsonl` with no database, see `db`) and publishes an
//! `ide` bus event. Owner actions do not pass the laws classifier. The owner is the principal, so no write
//! needs an approval.
//!
//! Routes (owner-only: the hub's gate in `hub_auth` covers every `/v1` path). The agent kind answers through
//! the agent's tools, the repo kind through Weft:
//!
//! | route | agent | repo | station |
//! |---|---|---|---|
//! | `GET /v1/ide/workspaces` | agents with a VM | project repos, on main | online workstations |
//! | `GET /v1/ide/workspaces/{id}/tree?path=&cursor=` | `ide_list` | main's files, 500 per page | `files list` |
//! | `GET /v1/ide/workspaces/{id}/file?path=` | `ide_read` (chunks) | main's file, text only, 1 MiB | `files read` (chunks) |
//! | `PUT /v1/ide/workspaces/{id}/file` `{path, text, expected_hash}` | `ide_write` | an owner edit that lands at once | `files write`, audited in both logs |
//! | `GET /v1/ide/workspaces/{id}/changes?base=` | `ide_changes` (chunks) | main against the landing before it (or `base`, a revision or change): Weft's diff | 501 (no checkpoints) |
//! | `GET /v1/ide/workspaces/{id}/checkpoints` | `ide_checkpoints` | what landed, newest first | `{checkpoints: [], reason}` |
//! | `POST /v1/ide/workspaces/{id}/checkpoints/{n}/restore` | `ide_restore` | 501 (revert the change in History) | 501 |
//! | `GET /v1/ide/workspaces/{id}/search?q=&glob=&max=` | `ide_search` | 501 | `files search` |
//! | `POST /v1/ide/workspaces/{id}/runs`, `GET .../runs/{run}`, `POST .../runs/{run}/stop` | `ide_run_*` | 501 | `run start/poll/stop` |
//! | `POST /v1/ide/workspaces/{id}/pulls` `{title, body?, base?}` | 400 | 410 (gone with the git host) | 400 |
//! | `POST /v1/ide/workspaces/{id}/branch` `{name}` | 400 | 410 | 400 |
//! | `GET /v1/ide/audit?workspace=&limit=` | (audit log) | (audit log) | (audit log) |
//!
//! A tell that fails maps to: 404 unknown agent, 409 no session, 504 no answer in time, 403 a tool the
//! agent's role does not grant, 400 a refusal (`ok: false`) with the agent's text, and 502 anything else. A
//! Weft answer keeps its status and message. A station answer maps to: 409 STOP ALL, not linked or draining,
//! 504 no answer in time, 400 a refusal. A write that finds the file moved answers 409
//! `{error: "conflict", current_hash}` in every kind.

use std::collections::BTreeSet;
use std::time::Duration;

use apz_http::{Request, Response, RouteDoc, Router};
use apz_ide::diff::{parse_unified, write_binary, write_unified};
use apz_ide::path::{PathError, check_dir, check_read, check_write};
use apz_ide::tree::PAGE_SIZE;
use apz_ide::workspace::WorkspaceId;
use apz_station::{FilesOp, Op, Os, RunOp};
use serde_json::{Value, json};

use crate::agents::{Agent, Status};
use crate::db::Db;
use crate::plaza::percent_decode;
use crate::state::App;
use crate::stations::link::LINK_CLOSED;

/// How long a tell waits for the agent: the harness's own cap (vsockd's `MAX_TELL_TIMEOUT_MS`).
pub const TELL_TIMEOUT_MS: u64 = 5_000;
/// The most chunks one assembled read or diff takes. The harness caps a diff at 8 MiB, so this is never reached.
const MAX_CHUNKS: usize = 1_024;
const DEFAULT_AUDIT_LIMIT: usize = 100;
const MAX_AUDIT_LIMIT: usize = 1_000;
/// The most text a repo file may be, in bytes: the harness's cap for an agent's file.
const MAX_TEXT: u64 = 1024 * 1024;
/// The most diff a repo changes call returns, in bytes: the harness's cap for an agent's diff.
const MAX_DIFF: usize = 8 * 1024 * 1024;
/// The newest commits a repo workspace lists as checkpoints.
const CHECKPOINT_LIMIT: usize = 50;
/// The most changed files a repo changes call diffs. The rest are left out, and the answer says `truncated`.
const MAX_CHANGED_FILES: usize = 300;
/// The context lines around each change in a repo changes diff: git's default.
const CHANGE_CONTEXT: usize = 3;
/// The bytes of a file looked at for a NUL, the way git decides a file is binary.
const BINARY_PROBE: usize = 8 * 1024;
/// Repo names the IDE never offers: the platform's own source repository, under the names it has.
const PLATFORM_REPOS: &[&str] = &["agentplaza", "agentplaza-mvp"];
/// How long a workstation gets to answer one IDE operation (its daemon's own cap for a run is shorter).
pub const STATION_TIMEOUT: Duration = Duration::from_secs(15);
/// The refusal of every IDE operation while STOP ALL holds the plaza.
const STOP_ALL_MESSAGE: &str =
    "STOP ALL holds the plaza: resume it before IDE work on a workstation";
/// Why a workstation has no checkpoints, changes or restore.
const NO_CHECKPOINTS: &str =
    "a workstation has no checkpoints: its work folder is not versioned by the hub";

type Reply = Result<Value, Response>;

/// What the routes need from the world: the agents' tool calls, the database (the audit lives there), the bus,
/// Weft and the projects' repos. The live hub implements it with the fleet and the app; the tests with a fake.
pub trait Host {
    /// One tool call on agent `cid`: the gateway's status and body (vsockd's `/tell` answer).
    fn tell(&self, cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String>;
    /// The database: the IDE audit is its `ide_audit` table, or the state directory's file without one.
    fn db(&self) -> &Db;
    /// Publishes one `ide` bus event.
    fn publish(&self, data: &Value);
    /// Weft, as the owner (the repo workspaces): one call under `weft serve`'s `/v1`, its status and body.
    fn weft(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), String>;
    /// The repo name (without the org) of every project that has one.
    fn project_repos(&self) -> Vec<String>;
    /// The workstations the IDE can open now: online, not revoked and not draining.
    fn stations(&self) -> Vec<StationSpace>;
    /// One operation on workstation `id` through its live link: the daemon's answer, or the reason it gave none.
    fn station_request(&self, id: &str, op: Op) -> Result<Value, String>;
    /// Whether STOP ALL holds the plaza. Every IDE operation on a workstation is refused while it does.
    fn frozen(&self) -> bool;
}

/// The live hub: tells go through the fleet, and the database, the bus, Weft and the projects are the app's.
struct Live<'a>(&'a App);

impl Host for Live<'_> {
    fn tell(&self, cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String> {
        let body = json!({ "name": name, "args": args, "timeout_ms": TELL_TIMEOUT_MS });
        crate::fleet::gateway(
            self.0,
            cid,
            "POST",
            &format!("/v1/agents/{cid}/tell"),
            Some(&body),
        )
    }

    fn db(&self) -> &Db {
        &self.0.db
    }

    fn publish(&self, data: &Value) {
        self.0.bus.publish("ide", data);
    }

    fn weft(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), String> {
        let client = self.0.weft.client("owner")?;
        let mut target = client.target.clone();
        if body.is_some() {
            target
                .headers
                .push(("content-type".into(), "application/json".into()));
        }
        let bytes = body.map(|b| b.to_string().into_bytes());
        apz_http::client::request(
            &target,
            method,
            &format!("{}{path}", client.prefix),
            bytes.as_deref(),
            Duration::from_secs(60),
        )
        .map(|reply| (reply.status, reply.body))
        .map_err(|err| err.to_string())
    }

    fn project_repos(&self) -> Vec<String> {
        self.0
            .projects
            .list()
            .into_iter()
            .filter_map(|project| project.repo)
            .collect()
    }

    fn stations(&self) -> Vec<StationSpace> {
        station_spaces(self.0)
    }

    fn station_request(&self, id: &str, op: Op) -> Result<Value, String> {
        station_op(self.0, id, op)
    }

    fn frozen(&self) -> bool {
        self.0.control.paused()
    }
}

/// An online workstation the IDE opens: its id (`w<seq>`), its name, and its OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationSpace {
    pub id: String,
    pub name: String,
    pub os: &'static str,
}

/// The workstations the IDE opens now: enrolled, linked, not revoked and not draining.
fn station_spaces(app: &App) -> Vec<StationSpace> {
    let now = apz_plaza::now_ms();
    app.stations
        .list()
        .into_iter()
        .filter(|station| {
            !station.revoked && !station.draining && app.stations.is_online(&station.id, now)
        })
        .map(|station| StationSpace {
            os: os_name(station.facts.os),
            id: station.id,
            name: station.name,
        })
        .collect()
}

fn os_name(os: Os) -> &'static str {
    match os {
        Os::Macos => "macos",
        Os::Windows => "windows",
        Os::Linux => "linux",
    }
}

/// One operation on workstation `id` through its live link, waiting up to [`STATION_TIMEOUT`]. A workstation
/// with no live link is "not linked".
pub fn station_op(app: &App, id: &str, op: Op) -> Result<Value, String> {
    match app.stations.live(id) {
        Some(link) => link.request(op, STATION_TIMEOUT),
        None => Err("the workstation is not linked".into()),
    }
}

/// Whether an IDE operation may reach workstation `id` now: STOP ALL holds nothing, and the station is one the
/// IDE opens. The terminal socket asks this before it upgrades.
pub fn station_gate(app: &App, id: &str) -> Result<(), Response> {
    check_station(&Live(app), id)
}

fn check_station(host: &dyn Host, id: &str) -> Result<(), Response> {
    if host.frozen() {
        return Err(Response::error(409, STOP_ALL_MESSAGE));
    }
    if host.stations().iter().any(|station| station.id == id) {
        Ok(())
    } else {
        Err(Response::error(
            404,
            &format!("no workspace {}", WorkspaceId::Station(id.to_string())),
        ))
    }
}

/// Whether a station's error means no answer came back at all: its link is down, it is draining, or it did not
/// answer in time. The terminal socket reads these as "unreachable" (the socket may retry), a refusal as "refused".
pub fn is_unreachable(message: &str) -> bool {
    message == LINK_CLOSED
        || message.contains("not linked")
        || message.contains("draining")
        || message.contains("did not answer in time")
}

/// A repo workspace the IDE opens: a project repo at one branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWs {
    pub name: String,
    pub git_ref: String,
}

impl RepoWs {
    /// The workspace id, `repo:<name>@<ref>`.
    fn id(&self) -> String {
        format!("repo:{}@{}", self.name, self.git_ref)
    }
}

/// A project repo at its default branch: what the workspace list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpace {
    pub name: String,
    pub default_branch: String,
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
    let output = body
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match status {
        200 => Ok(Answer {
            ok: body.get("ok").and_then(Value::as_bool).unwrap_or(false),
            output,
        }),
        403 => Err(Response::error(403, &output)),
        404 => Err(Response::error(404, &format!("no agent {cid}"))),
        409 => Err(Response::error(409, "the agent has no session")),
        504 => Err(Response::error(
            504,
            "the agent did not answer in time (it may be busy)",
        )),
        other => Err(Response::error(
            502,
            &format!("the agent's node answered {other}: {body}"),
        )),
    }
}

/// The tool's JSON answer. A refusal is 400 with the agent's text.
fn reply(answer: Answer) -> Reply {
    if !answer.ok {
        return Err(Response::error(400, &answer.output));
    }
    serde_json::from_str(&answer.output).map_err(|err| {
        Response::error(
            502,
            &format!("the agent answered with text that is not JSON: {err}"),
        )
    })
}

/// One tool call whose answer is JSON.
fn tool(host: &dyn Host, cid: u64, name: &str, args: &Value) -> Reply {
    reply(call(host, cid, name, args)?)
}

/// One Weft call as the owner, raw: its status and body. A transport failure is 503.
fn weft_raw(
    host: &dyn Host,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Vec<u8>), Response> {
    host.weft(method, path, body)
        .map_err(|err| Response::error(503, &format!("Weft: {err}")))
}

/// One Weft call as the owner, answered with JSON; Weft's refusal keeps its status and message.
fn weft_json(host: &dyn Host, method: &str, path: &str, body: Option<&Value>) -> Reply {
    let (status, bytes) = weft_raw(host, method, path, body)?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        Err(Response::error(
            status,
            value["error"]
                .as_str()
                .unwrap_or("Weft refused the request"),
        ))
    }
}

/// A file's bytes at a revision; `None` when it is not there.
fn weft_file(host: &dyn Host, rev: &str, path: &str) -> Result<Option<Vec<u8>>, Response> {
    let (status, bytes) = weft_raw(
        host,
        "GET",
        &format!(
            "/revisions/{rev}/file?path={}",
            weft::server::percent_encode(path)
        ),
        None,
    )?;
    match status {
        200 => Ok(Some(bytes)),
        404 => Ok(None),
        _ => {
            let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            Err(Response::error(
                status,
                value["error"].as_str().unwrap_or("Weft refused the read"),
            ))
        }
    }
}

/// The content hash Weft names a file by.
fn weft_hash(bytes: &[u8]) -> String {
    weft::hash::Hash::of(bytes).hex()
}

fn respond(result: Reply) -> Response {
    match result {
        Ok(value) => Response::json(200, &value),
        Err(response) => response,
    }
}

/// A created answer (201) or the failure.
fn created(result: Reply) -> Response {
    match result {
        Ok(value) => Response::json(201, &value),
        Err(response) => response,
    }
}

fn workspace_of(cid: u64) -> String {
    WorkspaceId::Agent(cid).to_string()
}

/// The workspace id of a route, parsed. A malformed id is 400.
fn parse_id(id: &str) -> Result<WorkspaceId, Response> {
    id.parse::<WorkspaceId>()
        .map_err(|err| Response::error(400, &err.to_string()))
}

/// Refuses the platform's own repository by name: it is never listed, and a direct id is refused too.
fn offered(name: &str) -> Result<(), Response> {
    if PLATFORM_REPOS.contains(&name) {
        Err(Response::error(
            403,
            "the platform's own repository is not offered in the IDE",
        ))
    } else {
        Ok(())
    }
}

/// A branch name the IDE takes: a ref git accepts, checked by the same rule that checks a repo id's ref.
fn check_branch(name: &str) -> Result<(), Response> {
    format!("repo:x@{name}")
        .parse::<WorkspaceId>()
        .map(|_| ())
        .map_err(|err| Response::error(400, &format!("not a branch name: {err}")))
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
        Response::error(
            500,
            &format!("the action happened but the audit log could not be written: {err}"),
        )
    })?;
    host.publish(&data);
    Ok(())
}

/// A path, cleaned by `check`. A refused path is 400 and nothing is sent to the agent or Weft.
fn checked(
    raw: Option<&str>,
    check: fn(&str) -> Result<String, PathError>,
) -> Result<String, Response> {
    check(raw.unwrap_or("")).map_err(|err| Response::error(400, &err.to_string()))
}

/// A string field of a JSON body.
fn arg_str<'a>(body: &'a Value, key: &str) -> Result<&'a str, Response> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Response::error(400, &format!("\"{key}\" must be a string")))
}

fn parse_u64(raw: &str, name: &str) -> Result<u64, Response> {
    raw.parse::<u64>()
        .map_err(|_| Response::error(400, &format!("\"{name}\" must be a whole number")))
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
        Status::Asleep => "asleep",
        Status::Dozing => "dozing",
    }
}

/// The repo workspaces the IDE opens: each project repo Weft has, on main. The platform's own repo is left out. Without
/// Weft there are none; a repo that cannot be read is skipped and logged, so one bad repo does not hide the rest.
pub fn open_repos(host: &dyn Host) -> Vec<RepoSpace> {
    let names: BTreeSet<String> = host
        .project_repos()
        .into_iter()
        .filter(|name| offered(name).is_ok())
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            match weft_raw(
                host,
                "GET",
                &format!("/repos/{}", weft::server::percent_encode(&name)),
                None,
            ) {
                Ok((200, _)) => Some(RepoSpace {
                    name,
                    default_branch: "main".into(),
                }),
                Ok((404, _)) => None,
                Ok((status, _)) => {
                    eprintln!("apid: the IDE cannot read the repo {name} in Weft ({status})");
                    None
                }
                Err(_) => None,
            }
        })
        .collect()
}

/// The workspace list: the agent workspaces (every agent that has a VM, in CID order), then the station ones,
/// then the repo ones. Each row is an `apz_ide::Workspace` (`id`, `kind`, `owner` as text, `ref`, `root`,
/// `writable`), which the desktop parses strictly, plus the console's `name` and `status`.
pub fn workspaces(agents: &[Agent], stations: &[StationSpace], repos: &[RepoSpace]) -> Value {
    let mut rows: Vec<&Agent> = agents
        .iter()
        .filter(|agent| agent.run.is_some() && agent.vm.is_some())
        .collect();
    rows.sort_by_key(|agent| agent.cid);
    let mut out: Vec<Value> = rows
        .into_iter()
        .map(|agent| {
            json!({
                "id": workspace_of(agent.cid),
                "kind": "agent",
                "owner": agent.cid.to_string(),
                "ref": null,
                "root": "/workspace",
                "name": agent.name,
                "writable": true,
                "status": status_name(agent.status),
            })
        })
        .collect();
    out.extend(stations.iter().map(|station| {
        json!({
            "id": format!("station:{}", station.id),
            "kind": "station",
            "owner": station.id,
            "ref": null,
            "root": "",
            "name": station.name,
            "os": station.os,
            "writable": true,
            "status": "online",
        })
    }));
    out.extend(repos.iter().map(|repo| {
        json!({
            "id": format!("repo:{}@{}", repo.name, repo.default_branch),
            "kind": "repo",
            "owner": repo.name,
            "ref": repo.default_branch,
            "root": repo.name,
            "name": repo.name,
            "writable": true,
            "status": "idle",
        })
    }));
    Value::Array(out)
}

/// The workspace an id names, checked to be one the IDE serves, then the core call on it: an agent with a VM,
/// a project repo that is not the platform's own, or an online workstation (not while STOP ALL holds).
fn with_workspace(
    app: &App,
    id: &str,
    on_agent: impl FnOnce(&dyn Host, u64) -> Reply,
    on_repo: impl FnOnce(&dyn Host, &RepoWs) -> Reply,
    on_station: impl FnOnce(&dyn Host, &str) -> Reply,
) -> Reply {
    let host = Live(app);
    match parse_id(id)? {
        WorkspaceId::Agent(cid) => {
            // An agent spawned since the last reconcile is looked up on its node (`place`), so its
            // workspace opens right away.
            if crate::lifecycle::place(app, cid).is_err() {
                return Err(Response::error(
                    404,
                    &format!("no workspace {}", workspace_of(cid)),
                ));
            }
            // Every agent workspace route comes through here: a sleeping agent wakes before its workspace is read.
            crate::sleep::awake_or_503(app, cid)?;
            on_agent(&host, cid)
        }
        WorkspaceId::Repo { name, git_ref } => {
            offered(&name)?;
            if !host.project_repos().contains(&name) {
                return Err(Response::error(
                    404,
                    &format!("no workspace repo:{name}@{git_ref}"),
                ));
            }
            on_repo(&host, &RepoWs { name, git_ref })
        }
        WorkspaceId::Station(station) => {
            check_station(&host, &station)?;
            on_station(&host, &station)
        }
    }
}

/// A route of the agent kind that the repo kind does not have yet.
fn not_for_repos(message: &str) -> Reply {
    Err(Response::error(501, message))
}

/// A route only the repo kind has.
fn repo_only() -> Reply {
    Err(Response::error(
        400,
        "a pull request and a branch are for repo workspaces",
    ))
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
    assemble_read(&path, |offset| {
        tool(
            host,
            cid,
            "ide_read",
            &json!({ "path": path, "offset": offset }),
        )
    })
}

/// A text file read in chunks: `next(offset)` gives each chunk (`hash`, `text`, `done`) until the last. The
/// whole text and its hash come back. A file that changes while it is read is 409.
fn assemble_read(path: &str, next: impl Fn(usize) -> Reply) -> Reply {
    let mut text = String::new();
    let mut hash: Option<String> = None;
    let mut offset = 0usize;
    for _ in 0..MAX_CHUNKS {
        let chunk = next(offset)?;
        let got = text_field(&chunk, "hash")?;
        if hash.is_none() {
            hash = Some(got.to_string());
        } else if hash.as_deref() != Some(got) {
            return Err(Response::error(
                409,
                "the file changed while it was read; read it again",
            ));
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
    Err(Response::error(
        502,
        "the file took too many chunks to read",
    ))
}

/// A write's body, checked: the path (cleaned, and refused for secrets and `.plaza/`), the text, and the hash
/// last read (`null` for a file that must not exist yet).
struct WriteArgs {
    path: String,
    text: String,
    expected: Value,
}

fn write_args(body: &Value) -> Result<WriteArgs, Response> {
    let path = checked(Some(arg_str(body, "path")?), check_write)?;
    let text = arg_str(body, "text")?.to_string();
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
    Ok(WriteArgs {
        path,
        text,
        expected,
    })
}

/// `PUT .../file` with `{path, text, expected_hash}`. `expected_hash` is the hash last read, or null for
/// a file that must not exist yet. A stale hash is 409 with the current one, and nothing is written. A
/// write is logged and published once it has happened.
pub fn write_file(host: &dyn Host, cid: u64, body: &Value) -> Reply {
    let WriteArgs {
        path,
        text,
        expected,
    } = write_args(body)?;
    let args = json!({ "path": path, "text": text, "expected_hash": expected });
    let answer = call(host, cid, "ide_write", &args)?;
    if !answer.ok {
        if let Some(now) = answer.output.strip_prefix("conflict: ") {
            let current = if now == "missing" {
                Value::Null
            } else {
                json!(now)
            };
            return Err(Response::json(
                409,
                &json!({ "error": "conflict", "current_hash": current }),
            ));
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
        let chunk = tool(
            host,
            cid,
            "ide_changes",
            &json!({ "base": base, "offset": offset }),
        )?;
        if offset == 0 {
            checkpoint = chunk["checkpoint"].clone();
        }
        let part = text_field(&chunk, "diff")?;
        diff.push_str(part);
        if chunk["done"] == json!(true) {
            let set = parse_unified(&diff).map_err(|err| {
                Response::error(502, &format!("the agent's diff does not parse: {err}"))
            })?;
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
    Err(Response::error(
        502,
        "the diff took too many chunks to read",
    ))
}

/// `GET .../checkpoints`.
pub fn checkpoints(host: &dyn Host, cid: u64) -> Reply {
    tool(host, cid, "ide_checkpoints", &json!({}))
}

/// `POST .../checkpoints/{n}/restore`. The harness takes a checkpoint of the current state first.
pub fn restore(host: &dyn Host, cid: u64, n: u64) -> Reply {
    let done = tool(host, cid, "ide_restore", &json!({ "n": n }))?;
    let ws = workspace_of(cid);
    let row = audit_row(
        &ws,
        "restore",
        None,
        json!({ "restored": n, "checkpoint": done["checkpoint"] }),
    );
    record(host, row, event(&ws, None, "restore"))?;
    Ok(done)
}

/// `GET .../search?q=&glob=&max=`. `q` is required.
pub fn search(
    host: &dyn Host,
    cid: u64,
    q: Option<&str>,
    glob: Option<&str>,
    max: Option<&str>,
) -> Reply {
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
        Some(value) => Some(value.as_u64().ok_or_else(|| {
            Response::error(400, "\"timeout_s\" must be a whole number of seconds")
        })?),
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
    record(
        host,
        audit_row(&ws, "run_stop", None, json!({ "run": run })),
        event(&ws, None, "run"),
    )?;
    Ok(done)
}

// The station kind. Every call goes to the workstation's daemon, which serves it inside the work folder with the
// IDE's own file, run and terminal code (`apz-ops`). The hub checks the paths first, as for the other kinds. A
// workstation has no checkpoints, so it has no changes, no restore and no branch.

/// The workspace id text of workstation `id`.
fn station_ws(id: &str) -> String {
    WorkspaceId::Station(id.to_string()).to_string()
}

/// A route of the station kind that is not served: the workstation has no checkpoints.
fn not_for_stations(message: &str) -> Reply {
    Err(Response::error(501, message))
}

/// The response for a station that gave no answer or refused: STOP ALL, a station that is not linked or is
/// draining, and no answer in time are 409 and 504; a refusal from the daemon is 400.
fn station_failure(message: String) -> Response {
    if message.contains("frozen by STOP ALL") {
        Response::error(409, STOP_ALL_MESSAGE)
    } else if message.contains("did not answer in time") {
        Response::error(504, &message)
    } else if is_unreachable(&message) {
        Response::error(409, &message)
    } else {
        Response::error(400, &message)
    }
}

/// One operation on station `id`: the daemon's answer, or the response for why there is none.
fn station_call(host: &dyn Host, id: &str, op: Op) -> Reply {
    host.station_request(id, op).map_err(station_failure)
}

/// The station audit row for one owner action on a workstation: kind `owner_ide`, with the action and its fields.
fn station_row(id: &str, action: &str, detail: Value) -> Value {
    let mut data = json!({ "action": action });
    if let (Some(map), Value::Object(detail)) = (data.as_object_mut(), detail) {
        map.extend(detail);
    }
    crate::stations::audit_row(Some(id), None, "owner_ide", "owner", data)
}

/// Writes the station audit row. A row that cannot be written is a 500, as for the IDE audit in [`record`].
fn record_station(host: &dyn Host, id: &str, action: &str, detail: Value) -> Result<(), Response> {
    host.db()
        .append_station_audit(&station_row(id, action, detail))
        .map_err(|err| {
            Response::error(
                500,
                &format!(
                    "the action happened but the station audit log could not be written: {err}"
                ),
            )
        })
}

/// `GET .../tree` of a station: one page of a folder in its work folder. A missing `path` is the work folder.
pub fn station_tree(host: &dyn Host, id: &str, path: Option<&str>, cursor: Option<&str>) -> Reply {
    let path = checked(path, check_dir)?;
    station_call(
        host,
        id,
        Op::Files(FilesOp::List {
            path,
            cursor: cursor.map(str::to_string),
        }),
    )
}

/// `GET .../file` of a station: the whole text file, assembled from the daemon's chunks.
pub fn station_read(host: &dyn Host, id: &str, path: Option<&str>) -> Reply {
    let path = checked(path, check_read)?;
    assemble_read(&path, |offset| {
        station_call(
            host,
            id,
            Op::Files(FilesOp::Read {
                path: path.clone(),
                offset: Some(offset as u64),
            }),
        )
    })
}

/// `PUT .../file` of a station: the file is written in the work folder, atomically. A stale `expected_hash` is 409
/// with the hash the file has now, and nothing is written. A write is logged in the IDE audit and the station audit.
pub fn station_write(host: &dyn Host, id: &str, body: &Value) -> Reply {
    let WriteArgs {
        path,
        text,
        expected,
    } = write_args(body)?;
    let op = Op::Files(FilesOp::Write {
        path: path.clone(),
        text,
        expected_hash: expected.as_str().map(str::to_string),
    });
    let done = match host.station_request(id, op) {
        Ok(done) => done,
        Err(message) => {
            if let Some(now) = message.strip_prefix("conflict: ") {
                let current = if now == "missing" {
                    Value::Null
                } else {
                    json!(now)
                };
                return Err(Response::json(
                    409,
                    &json!({ "error": "conflict", "current_hash": current }),
                ));
            }
            return Err(station_failure(message));
        }
    };
    let ws = station_ws(id);
    let row = audit_row(
        &ws,
        "write",
        Some(path.as_str()),
        json!({ "hash_before": expected, "hash_after": done["hash"], "checkpoint": null }),
    );
    record(host, row, event(&ws, Some(path.as_str()), "write"))?;
    record_station(
        host,
        id,
        "write",
        json!({ "path": path, "hash_before": expected, "hash_after": done["hash"] }),
    )?;
    Ok(done)
}

/// `GET .../search?q=&glob=&max=` of a station. `q` is required.
pub fn station_search(
    host: &dyn Host,
    id: &str,
    q: Option<&str>,
    glob: Option<&str>,
    max: Option<&str>,
) -> Reply {
    let q = q
        .filter(|q| !q.is_empty())
        .ok_or_else(|| Response::error(400, "\"q\" is required"))?;
    let max = match max {
        None => None,
        Some(raw) => Some(
            u32::try_from(parse_u64(raw, "max")?)
                .map_err(|_| Response::error(400, "\"max\" is too large"))?,
        ),
    };
    station_call(
        host,
        id,
        Op::Files(FilesOp::Search {
            q: q.to_string(),
            glob: glob.map(str::to_string),
            max,
        }),
    )
}

/// `GET .../checkpoints` of a station: none, with the reason.
pub fn station_checkpoints(_host: &dyn Host, _id: &str) -> Reply {
    Ok(json!({ "checkpoints": [], "reason": NO_CHECKPOINTS }))
}

/// `POST .../runs` of a station with `{command, timeout_s?}`. The run starts in the work folder and returns at once.
pub fn station_run_start(host: &dyn Host, id: &str, body: &Value) -> Reply {
    let command = arg_str(body, "command")?;
    if command.trim().is_empty() {
        return Err(Response::error(400, "\"command\" is empty"));
    }
    let timeout = match body.get("timeout_s") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    Response::error(400, "\"timeout_s\" must be a whole number of seconds")
                })?,
        ),
    };
    let done = station_call(
        host,
        id,
        Op::Run(RunOp::Start {
            command: command.to_string(),
            timeout_s: timeout,
        }),
    )?;
    let ws = station_ws(id);
    let detail = json!({ "run": done["run"], "command": command, "timeout_s": timeout });
    record(
        host,
        audit_row(&ws, "run", None, detail.clone()),
        event(&ws, None, "run"),
    )?;
    record_station(host, id, "run", detail)?;
    Ok(done)
}

/// `GET .../runs/{run}?offset=` of a station: the run's output from an offset, and its state.
pub fn station_run_poll(host: &dyn Host, id: &str, run: &str, offset: Option<&str>) -> Reply {
    let offset = offset.map(|raw| parse_u64(raw, "offset")).transpose()?;
    station_call(
        host,
        id,
        Op::Run(RunOp::Poll {
            run: run.to_string(),
            offset,
        }),
    )
}

/// `POST .../runs/{run}/stop` of a station.
pub fn station_run_stop(host: &dyn Host, id: &str, run: &str) -> Reply {
    let done = station_call(
        host,
        id,
        Op::Run(RunOp::Stop {
            run: run.to_string(),
        }),
    )?;
    let ws = station_ws(id);
    record(
        host,
        audit_row(&ws, "run_stop", None, json!({ "run": run })),
        event(&ws, None, "run"),
    )?;
    record_station(host, id, "run_stop", json!({ "run": run }))?;
    Ok(done)
}

// The repo kind: a project's repo in Weft, on main. Reads come from main's head; a write is the owner's edit, which
// lands on main at once (signed for the owner). Weft has no branches or pull requests: agents' work arrives as
// reviewed changes. Nothing runs in a VM.

/// The revision a repo workspace reads: its ref's head.
fn head_of(host: &dyn Host, ws: &RepoWs) -> Result<String, Response> {
    let repo = weft_json(
        host,
        "GET",
        &format!("/repos/{}", weft::server::percent_encode(&ws.name)),
        None,
    )?;
    Ok(repo["refs"][ws.git_ref.as_str()]
        .as_str()
        .unwrap_or("root")
        .to_string())
}

/// The last segment of an entry's path: its name.
fn entry_name(entry: &Value) -> &str {
    entry["path"]
        .as_str()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("")
}

/// `GET .../tree` of a repo: one page of a directory on main. Directories come first, then names
/// (case-insensitively, then exactly), as the agent kind lists them. The cursor is the next index.
pub fn repo_tree(host: &dyn Host, ws: &RepoWs, path: Option<&str>, cursor: Option<&str>) -> Reply {
    let path = checked(path, check_dir)?;
    let start = match cursor {
        None => 0,
        Some(raw) => raw.parse::<usize>().map_err(|_| {
            Response::error(400, "\"cursor\" must be a cursor from a previous page")
        })?,
    };
    let head = head_of(host, ws)?;
    let listing = weft_json(host, "GET", &format!("/revisions/{head}/files"), None)?;
    let dir = path.trim_matches('/').to_string();
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<Value> = Vec::new();
    for f in listing["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let full = f["path"].as_str().unwrap_or("");
        if full == dir && !dir.is_empty() {
            return Err(Response::error(400, &format!("{path} is not a directory")));
        }
        let Some(rest) = full.strip_prefix(&prefix) else {
            continue;
        };
        match rest.split_once('/') {
            Some((sub, _)) => {
                dirs.insert(format!("{prefix}{sub}"));
            }
            None => {
                let kind = if f["mode"].as_u64() == Some(0o120000) {
                    "symlink"
                } else {
                    "file"
                };
                entries.push(json!({ "path": full, "kind": kind, "size": f["size"].as_u64().unwrap_or(0), "hash": if kind == "file" { f["hash"].clone() } else { Value::Null }, "binary": false }));
            }
        }
    }
    if !dir.is_empty() && dirs.is_empty() && entries.is_empty() {
        return Err(Response::error(404, &format!("no directory {path}")));
    }
    entries
        .extend(dirs.into_iter().map(
            |d| json!({ "path": d, "kind": "dir", "size": 0, "hash": null, "binary": false }),
        ));
    entries.sort_by(|a, b| {
        let is_dir = |entry: &Value| entry["kind"] == json!("dir");
        is_dir(b)
            .cmp(&is_dir(a))
            .then_with(|| {
                entry_name(a)
                    .to_lowercase()
                    .cmp(&entry_name(b).to_lowercase())
            })
            .then_with(|| entry_name(a).cmp(entry_name(b)))
    });
    let end = (start + PAGE_SIZE).min(entries.len());
    let from = start.min(end);
    let next = (end < entries.len()).then(|| end.to_string());
    Ok(json!({ "entries": &entries[from..end], "next": next }))
}

/// `GET .../file` of a repo: a text file on main, with its hash. A binary file, one that is not UTF-8, one over
/// 1 MiB, and a directory are refused.
pub fn repo_read(host: &dyn Host, ws: &RepoWs, path: Option<&str>) -> Reply {
    let path = checked(path, check_read)?;
    let head = head_of(host, ws)?;
    let Some(bytes) = weft_file(host, &head, &path)? else {
        // A folder is not a file: say which it is.
        let listing = weft_json(host, "GET", &format!("/revisions/{head}/files"), None)?;
        let folder = format!("{}/", path.trim_end_matches('/'));
        let is_dir = listing["files"].as_array().is_some_and(|f| {
            f.iter()
                .any(|x| x["path"].as_str().is_some_and(|p| p.starts_with(&folder)))
        });
        return Err(if is_dir {
            Response::error(
                400,
                &format!("{path} is a directory; the IDE reads one file"),
            )
        } else {
            Response::error(404, &format!("no file {path}"))
        });
    };
    if bytes.len() as u64 > MAX_TEXT {
        return Err(Response::error(
            400,
            &format!(
                "{path} is {} bytes, over the {MAX_TEXT}-byte cap",
                bytes.len()
            ),
        ));
    }
    if bytes.contains(&0) {
        return Err(Response::error(
            400,
            &format!("{path} is binary (it holds a NUL byte); the IDE opens text only"),
        ));
    }
    let hash = weft_hash(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| Response::error(400, &format!("{path} is not UTF-8 text")))?;
    Ok(json!({ "path": path, "size": text.len(), "hash": hash, "text": text }))
}

/// `PUT .../file` of a repo: the owner's edit, which lands on main at once as a change of its own. A new file is
/// created when `expected_hash` is null; an existing one must still have `expected_hash`, else the answer is a 409
/// with the hash the file has now, and nothing lands. Answers `{path, hash, checkpoint}`, the checkpoint being the
/// landed revision.
pub fn repo_write(host: &dyn Host, ws: &RepoWs, body: &Value) -> Reply {
    let WriteArgs {
        path,
        text,
        expected,
    } = write_args(body)?;
    let head = head_of(host, ws)?;
    let now = weft_file(host, &head, &path)?.map(|b| weft_hash(&b));
    if now.as_deref() != expected.as_str() {
        return Err(Response::json(
            409,
            &json!({ "error": "conflict", "current_hash": now }),
        ));
    }
    let edit = json!({ "title": format!("owner edit: {path}"), "ref": ws.git_ref, "files": [{ "path": path, "text": text }] });
    let landed = weft_json(
        host,
        "POST",
        &format!("/repos/{}/edits", weft::server::percent_encode(&ws.name)),
        Some(&edit),
    )?;
    let hash = weft_hash(text.as_bytes());
    let checkpoint = landed["landed"].clone();
    let id = ws.id();
    let row = audit_row(
        &id,
        "write",
        Some(path.as_str()),
        json!({ "hash_before": expected, "hash_after": hash, "checkpoint": checkpoint, "change": landed["change"] }),
    );
    record(host, row, event(&id, Some(path.as_str()), "write"))?;
    Ok(json!({ "path": path, "hash": hash, "checkpoint": checkpoint }))
}

/// One side of a repo file at a revision. `Absent` is a file that is not there; `Binary` is one the IDE does not
/// show as lines (a NUL in its first 8 KiB, not UTF-8, or over 1 MiB), with its hash.
enum Side {
    Absent,
    Text(String),
    Binary(String),
}

fn repo_side(host: &dyn Host, rev: &str, path: &str) -> Result<Side, Response> {
    if rev == "root" {
        return Ok(Side::Absent);
    }
    let Some(bytes) = weft_file(host, rev, path)? else {
        return Ok(Side::Absent);
    };
    let hash = weft_hash(&bytes);
    if bytes.len() as u64 > MAX_TEXT || bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        return Ok(Side::Binary(hash));
    }
    Ok(match String::from_utf8(bytes) {
        Ok(text) => Side::Text(text),
        Err(_) => Side::Binary(hash),
    })
}

/// The diff text of one changed file between its two sides. Two text sides are diffed line by line; anything else
/// is a `Binary files ... differ` entry, and a binary file whose hash is the same on both sides is no change.
fn file_diff(old_path: Option<&str>, new_path: Option<&str>, old: &Side, new: &Side) -> String {
    match (old, new) {
        (Side::Binary(a), Side::Binary(b)) if a == b && old_path == new_path => String::new(),
        (Side::Binary(_), _) | (_, Side::Binary(_)) => write_binary(old_path, new_path),
        _ => {
            let text = |side: &Side| match side {
                Side::Text(t) => t.clone(),
                _ => String::new(),
            };
            write_unified(old_path, new_path, &text(old), &text(new), CHANGE_CONTEXT)
        }
    }
}

/// `GET .../changes?base=` of a repo, as a unified diff from the base to main's head. The base is `rev:<revision>`
/// or `change:<w-N>` (its landed revision); without one, main's last landing against the revision before it. At
/// most `MAX_CHANGED_FILES` files are diffed, and `truncated` says whether more changed. The checkpoint is main's
/// head.
pub fn repo_changes(host: &dyn Host, ws: &RepoWs, base: Option<&str>) -> Reply {
    let head = head_of(host, ws)?;
    if head == "root" {
        return Err(Response::error(
            404,
            &format!("nothing has landed on {}", ws.git_ref),
        ));
    }
    // `work_start` is an agent workspace's base; on a repo, as with none, it is the landing before main's head.
    let (label, base_rev) = match base.filter(|b| !matches!(*b, "" | "work_start")) {
        Some(raw) => {
            if let Some(rev) = raw.strip_prefix("rev:") {
                (raw.to_string(), rev.to_string())
            } else if let Some(change) = raw.strip_prefix("change:") {
                let c = weft_json(
                    host,
                    "GET",
                    &format!("/changes/{}", weft::server::percent_encode(change)),
                    None,
                )?;
                let landed = c["landed"]
                    .as_str()
                    .ok_or_else(|| Response::error(400, &format!("{change} has not landed")))?;
                (raw.to_string(), landed.to_string())
            } else {
                return Err(Response::error(
                    400,
                    "base must be rev:<revision> or change:<w-N>",
                ));
            }
        }
        None => {
            let rev = weft_json(host, "GET", &format!("/revisions/{head}"), None)?;
            let before = rev["base"].as_str().unwrap_or("root").to_string();
            (format!("rev:{before}"), before)
        }
    };
    let changed = weft_json(
        host,
        "GET",
        &format!(
            "/revisions/{head}/diff?base={}",
            weft::server::percent_encode(&base_rev)
        ),
        None,
    )?;
    let listed = changed["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let truncated = listed.len() > MAX_CHANGED_FILES;
    let mut diff = String::new();
    for item in listed.iter().take(MAX_CHANGED_FILES) {
        let name = item["path"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| Response::error(502, "Weft sent a changed file with no path"))?;
        let old = repo_side(host, &base_rev, name)?;
        let new = repo_side(host, &head, name)?;
        let old_path = (!matches!(old, Side::Absent)).then_some(name);
        let new_path = (!matches!(new, Side::Absent)).then_some(name);
        diff.push_str(&file_diff(old_path, new_path, &old, &new));
        if diff.len() > MAX_DIFF {
            return Err(Response::error(
                413,
                "the diff is over 8 MiB; compare smaller ranges",
            ));
        }
    }
    let set = parse_unified(&diff).map_err(|err| {
        Response::error(
            502,
            &format!("the diff built from Weft's files does not parse: {err}"),
        )
    })?;
    let stats = set.stats();
    Ok(
        json!({ "base": label, "checkpoint": head, "diff": diff, "files": set.files, "stats": stats, "truncated": truncated }),
    )
}

/// `GET .../checkpoints` of a repo: what landed on main, newest first. `n` is the index in that list.
pub fn repo_checkpoints(host: &dyn Host, ws: &RepoWs) -> Reply {
    let log = weft_json(
        host,
        "GET",
        &format!(
            "/repos/{}/log?q=landed()&limit={CHECKPOINT_LIMIT}",
            weft::server::percent_encode(&ws.name)
        ),
        None,
    )?;
    let rows: Vec<Value> = log["changes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(n, c)| json!({ "n": n, "label": c["title"], "created_ms": c["at_ms"], "sha": c["landed"], "change": c["change"] }))
        .collect();
    Ok(json!({ "checkpoints": rows }))
}

/// Why a repo workspace has no branches and no pull requests.
const NO_BRANCHES: &str = "Weft has no branches or pull requests: your edits land on main directly, and agents' work arrives as reviewed changes (History)";

/// `POST .../branch` of a repo: gone with the git host.
pub fn repo_branch(_host: &dyn Host, _ws: &RepoWs, body: &Value) -> Reply {
    check_branch(arg_str(body, "name")?)?;
    Err(Response::error(410, NO_BRANCHES))
}

/// `POST .../pulls` of a repo: gone with the git host.
pub fn repo_pull(_host: &dyn Host, _ws: &RepoWs, _body: &Value) -> Reply {
    Err(Response::error(410, NO_BRANCHES))
}

/// Milliseconds since the Unix epoch of an RFC 3339 instant, such as `2026-10-14T09:30:00Z` or one with
/// fractional seconds and an offset. None when it does not parse.
pub(crate) fn rfc3339_ms(text: &str) -> Option<u64> {
    let field =
        |range: std::ops::Range<usize>| text.get(range).and_then(|raw| raw.parse::<i64>().ok());
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    let bytes = text.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if separators
        .iter()
        .any(|&(at, want)| bytes.get(at) != Some(&want))
    {
        return None;
    }
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        // The first three digits, padded with zeros: `.25` is 250 ms.
        millis = fraction.as_bytes()[..digits]
            .iter()
            .copied()
            .take(3)
            .chain(std::iter::repeat(b'0'))
            .take(3)
            .fold(0i64, |acc, d| acc * 10 + i64::from(d - b'0'));
        rest = &fraction[digits..];
    }
    let offset_minutes = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match *rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hh = rest.get(1..3)?.parse::<i64>().ok()?;
            let mm = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (hh * 60 + mm)
        }
    };
    // Howard Hinnant's days_from_civil: the days since 1970-01-01 of a proleptic Gregorian date.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    u64::try_from(seconds * 1_000 + millis).ok()
}

/// `GET /v1/ide/audit?workspace=&limit=`: the newest rows first.
pub fn audit_rows(host: &dyn Host, workspace: Option<&str>, limit: Option<&str>) -> Reply {
    let limit = match limit {
        None => DEFAULT_AUDIT_LIMIT,
        Some(raw) => match parse_u64(raw, "limit")? {
            n @ 1..=1_000 => n as usize,
            _ => {
                return Err(Response::error(
                    400,
                    &format!("\"limit\" must be 1 to {MAX_AUDIT_LIMIT}"),
                ));
            }
        },
    };
    let rows = host
        .db()
        .ide_audit(workspace, limit as i64)
        .map_err(|err| Response::error(500, &format!("the audit log cannot be read: {err}")))?;
    Ok(Value::Array(rows))
}

/// A query value, percent-decoded. An empty value is absent.
fn query(req: &Request, name: &str) -> Option<String> {
    req.query_param(name)
        .map(percent_decode)
        .filter(|value| !value.is_empty())
}

fn body(req: &Request) -> Result<Value, Response> {
    req.json::<Value>()
        .map_err(|_| Response::error(400, "the body must be JSON"))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/ide/workspaces", |app, _, _| {
            Response::json(
                200,
                &workspaces(
                    &app.agents.list(),
                    &Live(app).stations(),
                    &open_repos(&Live(app)),
                ),
            )
        })
        .doc(RouteDoc::new("List the IDE's workspaces"))
        .get("/v1/ide/workspaces/{id}/tree", |app, req, p| {
            let (path, cursor) = (query(req, "path"), query(req, "cursor"));
            respond(with_workspace(
                app,
                p.get("id"),
                |host, cid| tree(host, cid, path.as_deref(), cursor.as_deref()),
                |host, ws| repo_tree(host, ws, path.as_deref(), cursor.as_deref()),
                |host, id| station_tree(host, id, path.as_deref(), cursor.as_deref()),
            ))
        })
        .doc(RouteDoc::new("List one folder of a workspace"))
        .get("/v1/ide/workspaces/{id}/file", |app, req, p| {
            let path = query(req, "path");
            respond(with_workspace(
                app,
                p.get("id"),
                |host, cid| read_file(host, cid, path.as_deref()),
                |host, ws| repo_read(host, ws, path.as_deref()),
                |host, id| station_read(host, id, path.as_deref()),
            ))
        })
        .doc(RouteDoc::new("Read a file in a workspace"))
        .route("PUT", "/v1/ide/workspaces/{id}/file", |app, req, p| {
            respond(body(req).and_then(|body| {
                with_workspace(
                    app,
                    p.get("id"),
                    |host, cid| write_file(host, cid, &body),
                    |host, ws| repo_write(host, ws, &body),
                    |host, id| station_write(host, id, &body),
                )
            }))
        })
        .doc(RouteDoc::new("Write a file in a workspace").errors(&[400, 409]))
        .get("/v1/ide/workspaces/{id}/changes", |app, req, p| {
            let base = query(req, "base");
            respond(with_workspace(
                app,
                p.get("id"),
                |host, cid| changes(host, cid, base.as_deref()),
                |host, ws| repo_changes(host, ws, base.as_deref()),
                |_, _| not_for_stations(NO_CHECKPOINTS),
            ))
        })
        .doc(RouteDoc::new("List the files changed since a base").errors(&[501]))
        .get("/v1/ide/workspaces/{id}/checkpoints", |app, _, p| {
            respond(with_workspace(
                app,
                p.get("id"),
                checkpoints,
                repo_checkpoints,
                station_checkpoints,
            ))
        })
        .doc(RouteDoc::new("List a workspace's checkpoints"))
        .post(
            "/v1/ide/workspaces/{id}/checkpoints/{n}/restore",
            |app, _, p| {
                respond(parse_u64(p.get("n"), "n").and_then(|n| {
                    with_workspace(
                        app,
                        p.get("id"),
                        |host, cid| restore(host, cid, n),
                        |_, _| {
                            not_for_repos(
                                "restore is not served for repo workspaces: use git revert",
                            )
                        },
                        |_, _| not_for_stations(NO_CHECKPOINTS),
                    )
                }))
            },
        )
        .doc(RouteDoc::new("Restore a workspace checkpoint").errors(&[501]))
        .get("/v1/ide/workspaces/{id}/search", |app, req, p| {
            let (q, glob, max) = (query(req, "q"), query(req, "glob"), query(req, "max"));
            respond(with_workspace(
                app,
                p.get("id"),
                |host, cid| search(host, cid, q.as_deref(), glob.as_deref(), max.as_deref()),
                |_, _| not_for_repos("search is not served for repo workspaces yet"),
                |host, id| station_search(host, id, q.as_deref(), glob.as_deref(), max.as_deref()),
            ))
        })
        .doc(RouteDoc::new("Search a workspace's files").errors(&[501]))
        .post("/v1/ide/workspaces/{id}/runs", |app, req, p| {
            respond(body(req).and_then(|body| {
                with_workspace(
                    app,
                    p.get("id"),
                    |host, cid| run_start(host, cid, &body),
                    |_, _| not_for_repos("runs are not served for repo workspaces yet"),
                    |host, id| station_run_start(host, id, &body),
                )
            }))
        })
        .doc(RouteDoc::new("Start a run in a workspace").errors(&[400, 501]))
        .get("/v1/ide/workspaces/{id}/runs/{run}", |app, req, p| {
            let offset = query(req, "offset");
            respond(with_workspace(
                app,
                p.get("id"),
                |host, cid| run_poll(host, cid, p.get("run"), offset.as_deref()),
                |_, _| not_for_repos("runs are not served for repo workspaces yet"),
                |host, id| station_run_poll(host, id, p.get("run"), offset.as_deref()),
            ))
        })
        .doc(RouteDoc::new("Read a run's output").errors(&[501]))
        .post("/v1/ide/workspaces/{id}/runs/{run}/stop", |app, _, p| {
            respond(with_workspace(
                app,
                p.get("id"),
                |host, cid| run_stop(host, cid, p.get("run")),
                |_, _| not_for_repos("runs are not served for repo workspaces yet"),
                |host, id| station_run_stop(host, id, p.get("run")),
            ))
        })
        .doc(RouteDoc::new("Stop a run").errors(&[501]))
        .post("/v1/ide/workspaces/{id}/pulls", |app, req, p| {
            created(body(req).and_then(|body| {
                with_workspace(
                    app,
                    p.get("id"),
                    |_, _| repo_only(),
                    |host, ws| repo_pull(host, ws, &body),
                    |_, _| repo_only(),
                )
            }))
        })
        .doc(
            RouteDoc::new("Open a pull request from a workspace branch")
                .status(201)
                .errors(&[400]),
        )
        .post("/v1/ide/workspaces/{id}/branch", |app, req, p| {
            created(body(req).and_then(|body| {
                with_workspace(
                    app,
                    p.get("id"),
                    |_, _| repo_only(),
                    |host, ws| repo_branch(host, ws, &body),
                    |_, _| repo_only(),
                )
            }))
        })
        .doc(
            RouteDoc::new("Create a branch in a workspace repo")
                .status(201)
                .errors(&[400]),
        )
        .get("/v1/ide/audit", |app, req, _| {
            respond(audit_rows(
                &Live(app),
                query(req, "workspace").as_deref(),
                query(req, "limit").as_deref(),
            ))
        })
        .doc(RouteDoc::new("List the IDE's audit log").errors(&[500]))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use apz_proto::chat::Usage;
    use apz_station::{HubFrame, StationFrame};

    use super::*;
    use crate::stations::link::StationLink;
    use crate::stations::link::tests as link_tests;

    type Answerer = Box<dyn Fn(&str, &Value) -> Result<(u16, Value), String>>;
    type WeftAnswerer = Box<dyn Fn(&str, &str, Option<&Value>) -> Result<(u16, Vec<u8>), String>>;

    /// A host whose tool calls and Weft calls `answer` decides. It records each call and each event.
    struct Fake {
        answer: Answerer,
        log: RefCell<Vec<(String, Value)>>,
        events: RefCell<Vec<Value>>,
        weft_answer: WeftAnswerer,
        /// (method, path, body or null) of each Weft call.
        weft_log: RefCell<Vec<(String, String, Value)>>,
        projects: Vec<String>,
        /// The workstations the IDE opens, and the live link each one answers through.
        stations: Vec<StationSpace>,
        links: Vec<(String, Arc<StationLink>)>,
        /// STOP ALL holds the plaza.
        stop_all: bool,
        /// No database: the audit is the state directory's file (`dir`).
        db: Db,
        dir: std::path::PathBuf,
    }

    impl Fake {
        fn new(
            tag: &str,
            answer: impl Fn(&str, &Value) -> Result<(u16, Value), String> + 'static,
        ) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            let dir =
                std::env::temp_dir().join(format!("apid-ide-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                answer: Box::new(answer),
                log: RefCell::default(),
                events: RefCell::default(),
                weft_answer: Box::new(|_, path, _| {
                    wjson(
                        404,
                        json!({ "error": format!("no Weft answer for {path}") }),
                    )
                }),
                weft_log: RefCell::default(),
                projects: Vec::new(),
                stations: Vec::new(),
                links: Vec::new(),
                stop_all: false,
                db: Db::files_only(&dir),
                dir,
            }
        }

        /// Weft answers with `answer`: (method, path, body).
        fn with_weft(
            mut self,
            answer: impl Fn(&str, &str, Option<&Value>) -> Result<(u16, Vec<u8>), String> + 'static,
        ) -> Self {
            self.weft_answer = Box::new(answer);
            self
        }

        /// The projects that have these repos (names without the org).
        fn with_projects(mut self, names: &[&str]) -> Self {
            self.projects = names.iter().map(|name| name.to_string()).collect();
            self
        }

        /// Workstation `id` is online and answers through `link`.
        fn with_station(mut self, id: &str, link: Arc<StationLink>) -> Self {
            self.stations.push(StationSpace {
                id: id.to_string(),
                name: "office".into(),
                os: "linux",
            });
            self.links.push((id.to_string(), link));
            self
        }

        /// STOP ALL holds the plaza.
        fn with_stop_all(mut self) -> Self {
            self.stop_all = true;
            self
        }

        /// The tool calls made so far.
        fn calls(&self) -> usize {
            self.log.borrow().len()
        }

        fn last_args(&self) -> Value {
            self.log
                .borrow()
                .last()
                .map(|(_, args)| args.clone())
                .unwrap_or(Value::Null)
        }

        /// The Weft calls made so far, as `METHOD path`.
        fn weft_paths(&self) -> Vec<String> {
            self.weft_log
                .borrow()
                .iter()
                .map(|(method, path, _)| format!("{method} {path}"))
                .collect()
        }

        /// The body of the last Weft call.
        fn last_weft_body(&self) -> Value {
            self.weft_log
                .borrow()
                .last()
                .map(|(_, _, body)| body.clone())
                .unwrap_or(Value::Null)
        }
    }

    impl Host for Fake {
        fn stations(&self) -> Vec<StationSpace> {
            self.stations.clone()
        }

        fn station_request(&self, id: &str, op: Op) -> Result<Value, String> {
            match self.links.iter().find(|(station, _)| station == id) {
                Some((_, link)) => link.request(op, Duration::from_secs(5)),
                None => Err("the workstation is not linked".into()),
            }
        }

        fn frozen(&self) -> bool {
            self.stop_all
        }

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

        fn weft(
            &self,
            method: &str,
            path: &str,
            body: Option<&Value>,
        ) -> Result<(u16, Vec<u8>), String> {
            self.weft_log.borrow_mut().push((
                method.to_string(),
                path.to_string(),
                body.cloned().unwrap_or(Value::Null),
            ));
            (self.weft_answer)(method, path, body)
        }

        fn project_repos(&self) -> Vec<String> {
            self.projects.clone()
        }
    }

    /// A workstation whose daemon is a script: `answer` gives each operation's result (an error is a refusal, and
    /// any other operation answers `null`). The operations the daemon was sent are kept in the returned log.
    fn scripted_station(
        answer: impl Fn(&Op) -> Result<Value, String> + Send + 'static,
    ) -> (Arc<StationLink>, Arc<Mutex<Vec<Op>>>) {
        let (link, daemon, _served) = link_tests::start("w3", Arc::new(link_tests::Accept));
        link_tests::greet(&link, &daemon);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            while let Ok(text) = daemon.hears.recv() {
                let Ok(HubFrame::Request { id, op }) = serde_json::from_str::<HubFrame>(&text)
                else {
                    continue;
                };
                let (ok, result, error) = match answer(&op) {
                    Ok(value) => (true, Some(value), None),
                    Err(message) => (false, None, Some(message)),
                };
                if op != Op::Facts {
                    log.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(op);
                }
                daemon.say(&StationFrame::Response {
                    id,
                    ok,
                    result,
                    error,
                });
            }
        });
        (link, seen)
    }

    /// A harness answer that succeeded, with `output` as its JSON text.
    fn ok(output: Value) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": true, "output": output.to_string() })))
    }

    /// A harness answer that refused, with `text` as its message.
    fn refused(text: &str) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": false, "output": text })))
    }

    /// A Weft answer with a JSON body.
    fn wjson(status: u16, value: Value) -> Result<(u16, Vec<u8>), String> {
        Ok((status, value.to_string().into_bytes()))
    }

    /// A Weft answer with raw bytes (a file).
    fn wbytes(bytes: &[u8]) -> Result<(u16, Vec<u8>), String> {
        Ok((200, bytes.to_vec()))
    }

    /// A repo in Weft whose main is at `head` with these files (path, text).
    fn weft_repo(
        head: &'static str,
        files: Vec<(&'static str, &'static [u8])>,
    ) -> impl Fn(&str, &str, Option<&Value>) -> Result<(u16, Vec<u8>), String> + 'static {
        move |method, path, body| {
            let listing: Vec<Value> = files.iter().map(|(p, b)| json!({ "path": p, "hash": weft_hash(b), "mode": 0o100644, "size": b.len() })).collect();
            match (method, path) {
                ("GET", "/repos/p1-plaza") => {
                    wjson(200, json!({ "name": "p1-plaza", "refs": { "main": head } }))
                }
                ("GET", p) if p == format!("/revisions/{head}/files") => {
                    wjson(200, json!({ "files": listing }))
                }
                ("GET", p) if p.starts_with(&format!("/revisions/{head}/file?path=")) => {
                    let want = crate::plaza::percent_decode(p.split_once("path=").unwrap().1);
                    match files.iter().find(|(f, _)| *f == want) {
                        Some((_, b)) => wbytes(b),
                        None => wjson(404, json!({ "error": "no such file" })),
                    }
                }
                ("POST", "/repos/p1-plaza/edits") => wjson(
                    200,
                    json!({ "change": "w-9", "landed": "w-9.l", "body": body }),
                ),
                _ => wjson(
                    404,
                    json!({ "error": format!("unexpected {method} {path}") }),
                ),
            }
        }
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
            caps: Vec::new(),
        }
    }

    fn repo(name: &str, branch: &str) -> RepoWs {
        RepoWs {
            name: name.into(),
            git_ref: branch.into(),
        }
    }

    #[test]
    fn workspace_ids_parse_and_the_platform_repo_is_refused() {
        assert_eq!(parse_id("agent:12").ok(), Some(WorkspaceId::Agent(12)));
        assert_eq!(status_of(parse_id("agent:012")), 400);
        assert_eq!(status_of(parse_id("nope")), 400);
        assert_eq!(status_of(parse_id("repo:proj-a@main")), 200);
        assert_eq!(status_of(offered("agentplaza")), 403);
        assert_eq!(status_of(offered("agentplaza-mvp")), 403);
        assert_eq!(status_of(offered("p1-plaza-web")), 200);
        assert_eq!(workspace_of(12), "agent:12");
    }

    #[test]
    fn workspaces_list_agents_with_a_vm_then_repos_at_their_default_branch() {
        let agents = vec![
            agent(42, Some("run-a"), Some(3), Status::Working),
            agent(7, None, None, Status::Offline),
            agent(9, Some("run-a"), Some(1), Status::Idle),
        ];
        let repos = vec![RepoSpace {
            name: "p1-plaza".into(),
            default_branch: "main".into(),
        }];
        let rows = workspaces(&agents, &[], &repos);
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["id"], json!("agent:9"));
        assert_eq!(rows[0]["status"], json!("idle"));
        assert_eq!(rows[1]["id"], json!("agent:42"));
        assert_eq!(rows[1]["status"], json!("working"));
        assert_eq!(rows[1]["owner"], json!("42"));
        // Every row parses as the IDE's own workspace type, which the desktop reads strictly.
        for row in rows.iter() {
            serde_json::from_value::<apz_ide::Workspace>(row.clone())
                .expect("an apz_ide::Workspace");
        }
        assert_eq!(rows[1]["kind"], json!("agent"));
        assert_eq!(rows[1]["writable"], json!(true));
        assert_eq!(
            rows[2],
            json!({
                "id": "repo:p1-plaza@main",
                "kind": "repo",
                "owner": "p1-plaza",
                "ref": "main",
                "root": "p1-plaza",
                "name": "p1-plaza",
                "writable": true,
                "status": "idle",
            })
        );
    }

    #[test]
    fn open_repos_lists_the_project_repos_weft_has_and_never_the_platform_one() {
        let fake = Fake::new("repos", |_, _| ok(json!({})))
            .with_projects(&["p1-plaza", "agentplaza", "p2-gone"])
            .with_weft(|_, path, _| match path {
                "/repos/p1-plaza" => wjson(
                    200,
                    json!({ "name": "p1-plaza", "refs": { "main": "w-1.l" } }),
                ),
                _ => wjson(404, json!({ "error": "no such repo" })),
            });
        assert_eq!(
            open_repos(&fake),
            vec![RepoSpace {
                name: "p1-plaza".into(),
                default_branch: "main".into()
            }]
        );
        assert!(
            !fake.weft_paths().iter().any(|p| p.contains("agentplaza")),
            "the platform's repo is never asked for"
        );
    }

    #[test]
    fn repo_tree_lists_main_with_folders_first_and_pages() {
        let fake = Fake::new("rtree", |_, _| ok(json!({}))).with_weft(weft_repo(
            "w-3.l",
            vec![
                ("docs/b.md", b"b"),
                ("docs/a.md", b"a"),
                ("docs/guides/x.md", b"x"),
                ("README.md", b"r"),
            ],
        ));
        let ws = repo("p1-plaza", "main");
        let page = repo_tree(&fake, &ws, Some("docs/"), None).unwrap();
        let names: Vec<&str> = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["docs/guides", "docs/a.md", "docs/b.md"]);
        assert_eq!(page["entries"][0]["kind"], "dir");
        assert_eq!(page["entries"][1]["hash"], json!(weft_hash(b"a")));
        let root = repo_tree(&fake, &ws, None, None).unwrap();
        assert_eq!(root["entries"].as_array().unwrap().len(), 2);
        assert_eq!(
            status_of(repo_tree(&fake, &ws, Some("docs"), Some("abc"))),
            400
        );
        assert_eq!(status_of(repo_tree(&fake, &ws, Some("../x"), None)), 400);
        assert_eq!(
            status_of(repo_tree(&fake, &ws, Some("README.md"), None)),
            400
        );
        assert_eq!(status_of(repo_tree(&fake, &ws, Some("nope"), None)), 404);
    }

    #[test]
    fn repo_read_returns_text_with_its_hash_and_refuses_what_is_not_text() {
        static HUGE: [u8; 1_100_000] = [b'a'; 1_100_000];
        let fake = Fake::new("rread", |_, _| ok(json!({}))).with_weft(weft_repo(
            "w-3.l",
            vec![
                ("notes/a.txt", b"hello\n"),
                ("bin.dat", b"a\0b"),
                ("huge.txt", &HUGE),
                ("docs/x.md", b"x"),
            ],
        ));
        let ws = repo("p1-plaza", "main");
        let file = repo_read(&fake, &ws, Some("notes/a.txt")).unwrap();
        assert_eq!(file["text"], "hello\n");
        assert_eq!(file["hash"], json!(weft_hash(b"hello\n")));
        assert_eq!(status_of(repo_read(&fake, &ws, Some("bin.dat"))), 400);
        assert_eq!(status_of(repo_read(&fake, &ws, Some("huge.txt"))), 400);
        assert_eq!(status_of(repo_read(&fake, &ws, Some("docs"))), 400);
        assert_eq!(status_of(repo_read(&fake, &ws, Some("missing.txt"))), 404);
        assert_eq!(
            status_of(repo_read(&fake, &ws, Some(".env"))),
            400,
            "a secret file is refused before Weft"
        );
    }

    #[test]
    fn a_repo_write_lands_as_the_owners_edit_and_a_stale_hash_is_a_conflict() {
        let fake = Fake::new("rwrite", |_, _| ok(json!({})))
            .with_weft(weft_repo("w-3.l", vec![("notes/a.txt", b"hello\n")]));
        let ws = repo("p1-plaza", "main");
        let body = json!({ "path": "notes/a.txt", "text": "hello, world\n", "expected_hash": weft_hash(b"hello\n") });
        let done = repo_write(&fake, &ws, &body).unwrap();
        assert_eq!(done["checkpoint"], "w-9.l");
        assert_eq!(done["hash"], json!(weft_hash(b"hello, world\n")));
        let sent = fake.last_weft_body();
        assert_eq!(sent["title"], "owner edit: notes/a.txt");
        assert_eq!(
            sent["files"][0],
            json!({ "path": "notes/a.txt", "text": "hello, world\n" })
        );
        assert!(fake.events.borrow().iter().any(|e| e["kind"] == "write"));
        // A stale hash: nothing lands, and the answer names the hash the file has now.
        let stale = json!({ "path": "notes/a.txt", "text": "x", "expected_hash": "old" });
        let response = repo_write(&fake, &ws, &stale).unwrap_err();
        assert_eq!(response.status, 409);
        assert_eq!(
            body_of(response)["current_hash"],
            json!(weft_hash(b"hello\n"))
        );
        // A new file must be new.
        let create = json!({ "path": "notes/a.txt", "text": "x", "expected_hash": null });
        assert_eq!(status_of(repo_write(&fake, &ws, &create)), 409);
        let new = json!({ "path": "notes/b.txt", "text": "b", "expected_hash": null });
        assert_eq!(repo_write(&fake, &ws, &new).unwrap()["checkpoint"], "w-9.l");
        for path in ["../x", ".env"] {
            assert_eq!(
                status_of(repo_write(
                    &fake,
                    &ws,
                    &json!({ "path": path, "text": "x", "expected_hash": null })
                )),
                400,
                "{path}"
            );
        }
    }

    #[test]
    fn changes_diff_main_against_the_landing_before_or_a_named_base() {
        let fake = Fake::new("rchanges", |_, _| ok(json!({}))).with_weft(|_, path, _| match path {
            "/repos/p1-plaza" => wjson(200, json!({ "refs": { "main": "w-4.l" } })),
            "/revisions/w-4.l" => wjson(200, json!({ "id": "w-4.l", "base": "w-3.l" })),
            "/changes/w-2" => wjson(200, json!({ "landed": "w-2.l" })),
            p if p.starts_with("/revisions/w-4.l/diff?base=") => wjson(200, json!({ "files": [{ "path": "src/a.rs" }, { "path": "logo.png", "binary": true }] })),
            p if p.starts_with("/revisions/w-4.l/file?path=src") => wbytes(b"fn a() {}\nfn b() {}\n"),
            p if p.starts_with("/revisions/w-3.l/file?path=src") || p.starts_with("/revisions/w-2.l/file?path=src") => wbytes(b"fn a() {}\n"),
            p if p.starts_with("/revisions/w-4.l/file?path=logo") => wbytes(b"\x89PNG\0new"),
            p if p.contains("/file?path=logo") => wbytes(b"\x89PNG\0old"),
            _ => wjson(404, json!({ "error": "no" })),
        });
        let ws = repo("p1-plaza", "main");
        let changes = repo_changes(&fake, &ws, None).unwrap();
        assert_eq!(changes["base"], "rev:w-3.l");
        assert_eq!(changes["checkpoint"], "w-4.l");
        assert_eq!(
            repo_changes(&fake, &ws, Some("work_start")).unwrap()["base"],
            "rev:w-3.l",
            "the IDE's default base is the landing before, as with none"
        );
        let diff = changes["diff"].as_str().unwrap();
        assert!(
            diff.contains("+fn b() {}") && diff.contains("Binary files"),
            "{diff}"
        );
        assert_eq!(
            repo_changes(&fake, &ws, Some("change:w-2")).unwrap()["base"],
            "change:w-2"
        );
        assert!(
            fake.weft_paths()
                .iter()
                .any(|p| p.contains("diff?base=w-2.l"))
        );
        assert_eq!(status_of(repo_changes(&fake, &ws, Some("branch:dev"))), 400);
    }

    #[test]
    fn checkpoints_are_what_landed_newest_first_and_branches_and_pulls_are_gone() {
        let fake = Fake::new("rcheck", |_, _| ok(json!({}))).with_weft(|_, path, _| match path {
            p if p.starts_with("/repos/p1-plaza/log") => wjson(200, json!({ "changes": [{ "change": "w-4", "title": "Faster queue", "at_ms": 20, "landed": "w-4.l" }, { "change": "w-3", "title": "Queue", "at_ms": 10, "landed": "w-3.l" }] })),
            _ => wjson(404, json!({})),
        });
        let ws = repo("p1-plaza", "main");
        let rows = repo_checkpoints(&fake, &ws).unwrap();
        assert_eq!(
            rows["checkpoints"][0],
            json!({ "n": 0, "label": "Faster queue", "created_ms": 20, "sha": "w-4.l", "change": "w-4" })
        );
        assert_eq!(
            status_of(repo_branch(&fake, &ws, &json!({ "name": "dev" }))),
            410
        );
        assert_eq!(
            status_of(repo_branch(&fake, &ws, &json!({ "name": "../bad" }))),
            400
        );
        assert_eq!(
            status_of(repo_pull(&fake, &ws, &json!({ "title": "x" }))),
            410
        );
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
    fn read_assembles_chunks_and_refuses_a_file_that_changes_while_it_is_read() {
        // The chunks are joined into the whole text, with the file's hash and size.
        {
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
        // A file whose hash changes between chunks is refused as a conflict.
        {
            let fake = Fake::new("moving", |_, args| {
                let offset = args["offset"].as_u64().unwrap();
                let (hash, text, done) = if offset == 0 {
                    ("h1", "abcd", false)
                } else {
                    ("h2", "efgh", true)
                };
                ok(
                    json!({ "path": "a.txt", "size": 8, "hash": hash, "offset": offset, "text": text, "done": done }),
                )
            });
            assert_eq!(status_of(read_file(&fake, 12, Some("a.txt"))), 409);
        }
    }

    #[test]
    fn bad_paths_are_refused_before_any_tell() {
        let fake = Fake::new("refuse", |_, _| ok(json!({})));
        for raw in ["", "../x", "/etc/passwd", ".env", "keys/id_rsa.pem"] {
            assert_eq!(
                status_of(read_file(&fake, 12, Some(raw))),
                400,
                "read {raw:?}"
            );
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
    fn write_conflicts_are_not_logged_and_a_missing_file_says_so() {
        // A stale write is a conflict: 409 with the current hash, and it is neither audited nor published.
        {
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
        // A write to a file that is gone is a conflict with no current hash.
        {
            let fake = Fake::new("missing", |_, _| refused("conflict: missing"));
            let body = json!({ "path": "gone.txt", "text": "x", "expected_hash": "h0" });
            let response = write_file(&fake, 12, &body).unwrap_err();
            assert_eq!(response.status, 409);
            assert_eq!(body_of(response)["current_hash"], Value::Null);
        }
    }

    #[test]
    fn a_write_is_audited_and_published_and_survives_a_reopen() {
        let fake = Fake::new("write", |_, _| {
            ok(json!({ "path": "docs/a.txt", "hash": "new", "checkpoint": 7 }))
        });
        let body = json!({ "path": "docs/a.txt", "text": "two", "expected_hash": "old" });
        let done = write_file(&fake, 12, &body).unwrap();
        assert_eq!(done["hash"], json!("new"));
        assert_eq!(fake.calls(), 1, "the write goes through its tool");
        assert_eq!(fake.last_args()["expected_hash"], json!("old"));

        let rows = fake.db.ide_audit(Some("agent:12"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kind"], json!("write"));
        assert_eq!(rows[0]["actor"], json!("owner"));
        assert_eq!(rows[0]["path"], json!("docs/a.txt"));
        assert_eq!(rows[0]["hash_before"], json!("old"));
        assert_eq!(rows[0]["hash_after"], json!("new"));
        assert_eq!(rows[0]["checkpoint"], json!(7));
        assert!(rows[0]["at_ms"].as_u64().is_some());
        assert_eq!(
            fake.events.borrow()[0],
            json!({ "workspace": "agent:12", "path": "docs/a.txt", "kind": "write" })
        );

        // The row is on disk: a fresh handle on the same file reads it back.
        let reopened = Db::files_only(&fake.dir);
        assert_eq!(reopened.ide_audit(None, 10).unwrap().len(), 1);
    }

    /// A two-file unified diff: one change to `a.txt` and one new file `b.txt`.
    const SAMPLE_DIFF: &str = "diff --git a/a.txt b/a.txt
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

    #[test]
    fn changes_chunk_the_diff_parse_its_files_and_forward_the_base() {
        // The diff comes in chunks and is assembled whole; its files and stats are parsed from it.
        {
            const DIFF: &str = SAMPLE_DIFF;
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
        // An empty diff has no files.
        {
            let fake = Fake::new("empty", |_, _| {
                ok(
                    json!({ "base": "work_start", "checkpoint": 1, "offset": 0, "total": 0, "diff": "", "done": true }),
                )
            });
            let reply = changes(&fake, 12, None).unwrap();
            assert_eq!(reply["files"], json!([]));
            assert_eq!(reply["stats"]["files"], json!(0));
        }
        // The base is forwarded to the tool.
        {
            let fake = Fake::new("base", |_, _| {
                ok(
                    json!({ "base": "checkpoint:4", "checkpoint": 4, "offset": 0, "total": 0, "diff": "", "done": true }),
                )
            });
            assert!(changes(&fake, 12, Some("checkpoint:4")).is_ok());
            assert_eq!(
                fake.last_args(),
                json!({ "base": "checkpoint:4", "offset": 0 })
            );
        }
    }

    #[test]
    fn restore_is_audited() {
        let fake = Fake::new("restore", |_, _| {
            ok(json!({ "restored": 3, "checkpoint": 9 }))
        });
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
        let started = run_start(
            &fake,
            12,
            &json!({ "command": "cargo test", "timeout_s": 30 }),
        )
        .unwrap();
        assert_eq!(started["run"], json!("run-1"));
        assert_eq!(
            fake.last_args(),
            json!({ "command": "cargo test", "timeout_s": 30 })
        );
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
        assert_eq!(
            status_of(run_start(&fake, 12, &json!({ "command": "   " }))),
            400
        );
        assert_eq!(
            status_of(run_start(
                &fake,
                12,
                &json!({ "command": "ls", "timeout_s": "soon" })
            )),
            400
        );
        assert_eq!(status_of(run_start(&fake, 12, &json!({}))), 400);
        assert_eq!(fake.calls(), 0);
        assert!(fake.db.ide_audit(None, 10).unwrap().is_empty());
    }

    #[test]
    fn search_needs_a_query_and_forwards_the_rest() {
        let fake = Fake::new("search", |_, _| {
            ok(json!({ "matches": [], "truncated": false }))
        });
        assert_eq!(status_of(search(&fake, 12, None, None, None)), 400);
        assert_eq!(status_of(search(&fake, 12, Some(""), None, None)), 400);
        assert_eq!(fake.calls(), 0);
        assert!(search(&fake, 12, Some("needle"), Some("*.rs"), Some("5")).is_ok());
        assert_eq!(
            fake.last_args(),
            json!({ "q": "needle", "glob": "*.rs", "max": 5 })
        );
        assert_eq!(
            status_of(search(&fake, 12, Some("needle"), None, Some("many"))),
            400
        );
    }

    #[test]
    fn a_failed_tell_maps_to_its_status_and_a_refusal_keeps_its_text() {
        // Each failed tell maps to the status of its cause.
        {
            type Tell = fn(&str, &Value) -> Result<(u16, Value), String>;
            let cases: [(&str, Tell, u16); 7] = [
                ("timeout", |_, _| Ok((504, json!({ "error": "x" }))), 504),
                ("no session", |_, _| Ok((409, json!({ "error": "x" }))), 409),
                (
                    "unknown cid",
                    |_, _| Ok((404, json!({ "error": "x" }))),
                    404,
                ),
                (
                    "role",
                    |_, _| {
                        Ok((
                            403,
                            json!({ "ok": false, "output": "role does not grant ide_list" }),
                        ))
                    },
                    403,
                ),
                (
                    "node down",
                    |_, _| Err("node n2 is offline".to_string()),
                    502,
                ),
                (
                    "refusal",
                    |_, _| refused("denied: secret files are not available"),
                    400,
                ),
                (
                    "not json",
                    |_, _| Ok((200, json!({ "ok": true, "output": "nope" }))),
                    502,
                ),
            ];
            for (tag, answer, want) in cases {
                let fake = Fake::new(tag, answer);
                assert_eq!(status_of(tree(&fake, 12, None, None)), want, "{tag}");
            }
        }
        // A refusal's text is the error body, unchanged.
        {
            let fake = Fake::new("text", |_, _| {
                refused("denied: secret files are not available")
            });
            let response = tree(&fake, 12, None, None).unwrap_err();
            assert_eq!(
                body_of(response)["error"],
                json!("denied: secret files are not available")
            );
        }
    }

    #[test]
    fn the_audit_log_filters_and_checks_its_limit() {
        let fake = Fake::new("auditq", |_, _| ok(json!({ "run": "run-1" })));
        fake.db
            .append_ide_audit(
                &json!({ "at_ms": 1, "workspace": "agent:12", "kind": "write", "n": 1 }),
            )
            .unwrap();
        fake.db
            .append_ide_audit(
                &json!({ "at_ms": 2, "workspace": "agent:40", "kind": "write", "n": 2 }),
            )
            .unwrap();
        fake.db
            .append_ide_audit(
                &json!({ "at_ms": 3, "workspace": "agent:12", "kind": "restore", "n": 3 }),
            )
            .unwrap();

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

    // The station kind: a workstation's work folder, its runs and its audit, through its daemon.

    #[test]
    fn online_stations_are_listed_between_the_agents_and_the_repos() {
        let agents = vec![agent(3, Some("r"), Some(7), Status::Idle)];
        let stations = vec![StationSpace {
            id: "w3".into(),
            name: "office".into(),
            os: "linux",
        }];
        let repos = vec![RepoSpace {
            name: "plaza-web".into(),
            default_branch: "main".into(),
        }];
        let rows = workspaces(&agents, &stations, &repos);
        let ids: Vec<&str> = rows
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|row| row["id"].as_str())
            .collect();
        assert_eq!(ids, ["agent:3", "station:w3", "repo:plaza-web@main"]);
        assert_eq!(rows[1]["kind"], "station");
        assert_eq!(rows[1]["owner"], "w3");
        assert_eq!(rows[1]["name"], "office");
        assert_eq!(rows[1]["os"], "linux");
        assert_eq!(rows[1]["status"], "online");
    }

    #[test]
    fn a_station_lists_and_reads_its_work_folder_through_its_daemon() {
        let (link, seen) = scripted_station(|op| match op {
            Op::Files(FilesOp::List { .. }) => {
                Ok(json!({ "entries": [{ "path": "docs", "kind": "dir" }], "next": null }))
            }
            Op::Files(FilesOp::Read { .. }) => {
                Ok(json!({ "path": "docs/a.txt", "hash": "h1", "text": "hi", "done": true }))
            }
            _ => Ok(Value::Null),
        });
        let host = Fake::new("station-read", |_, _| Err("no tool".into())).with_station("w3", link);

        let tree = station_tree(&host, "w3", Some("docs"), Some("0")).expect("listed");
        assert_eq!(tree["entries"][0]["path"], "docs");
        let text = station_read(&host, "w3", Some("docs/a.txt")).expect("read");
        assert_eq!(
            (text["text"].as_str(), text["hash"].as_str()),
            (Some("hi"), Some("h1"))
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                Op::Files(FilesOp::List {
                    path: "docs".into(),
                    cursor: Some("0".into())
                }),
                Op::Files(FilesOp::Read {
                    path: "docs/a.txt".into(),
                    offset: Some(0)
                }),
            ]
        );
        assert_eq!(status_of(station_read(&host, "w3", Some("../secret"))), 400);
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "a refused path never reaches the daemon"
        );
    }

    #[test]
    fn a_station_write_is_audited_in_both_logs_and_a_stale_one_is_a_conflict() {
        let (link, seen) = scripted_station(|op| match op {
            Op::Files(FilesOp::Write { expected_hash, .. })
                if expected_hash.as_deref() == Some("h1") =>
            {
                Ok(json!({ "path": "a.txt", "hash": "h2" }))
            }
            Op::Files(FilesOp::Write { .. }) => Err("conflict: h9".into()),
            _ => Ok(Value::Null),
        });
        let host =
            Fake::new("station-write", |_, _| Err("no tool".into())).with_station("w3", link);

        let saved = station_write(
            &host,
            "w3",
            &json!({ "path": "a.txt", "text": "new", "expected_hash": "h1" }),
        )
        .expect("saved");
        assert_eq!(saved["hash"], "h2");
        let Err(stale) = station_write(
            &host,
            "w3",
            &json!({ "path": "a.txt", "text": "newer", "expected_hash": "h0" }),
        ) else {
            panic!("a stale write is refused");
        };
        assert_eq!(stale.status, 409);
        assert_eq!(body_of(stale)["current_hash"], "h9");

        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "both writes reached the daemon"
        );
        let rows = host
            .db
            .ide_audit(Some("station:w3"), 10)
            .expect("the audit reads");
        assert_eq!(rows.len(), 1, "only the write that happened is audited");
        assert_eq!(rows[0]["kind"], "write");
        assert_eq!(rows[0]["path"], "a.txt");
        assert_eq!(rows[0]["hash_before"], "h1");
        assert_eq!(rows[0]["hash_after"], "h2");
        let station = host
            .db
            .station_audit(Some("w3"), 10)
            .expect("the station audit reads");
        assert_eq!(station.len(), 1);
        assert_eq!(station[0]["kind"], "owner_ide");
        assert_eq!(station[0]["action"], "write");
        assert_eq!(station[0]["path"], "a.txt");
        assert_eq!(host.events.borrow()[0]["workspace"], "station:w3");
    }

    #[test]
    fn a_station_run_starts_polls_and_stops_on_its_daemon_and_is_audited() {
        let (link, seen) = scripted_station(|op| match op {
            Op::Run(RunOp::Start { .. }) => Ok(json!({ "run": "run-1" })),
            Op::Run(RunOp::Poll { .. }) => Ok(
                json!({ "run": "run-1", "output": "hi\n", "next": 3, "done": true, "exit_code": 0, "timed_out": false }),
            ),
            Op::Run(RunOp::Stop { .. }) => Ok(json!({ "run": "run-1", "stopped": true })),
            _ => Ok(Value::Null),
        });
        let host = Fake::new("station-run", |_, _| Err("no tool".into())).with_station("w3", link);

        assert_eq!(
            status_of(station_run_start(&host, "w3", &json!({ "command": "  " }))),
            400,
            "an empty command is refused"
        );
        let started = station_run_start(&host, "w3", &json!({ "command": "ls", "timeout_s": 30 }))
            .expect("started");
        assert_eq!(started["run"], "run-1");
        let polled = station_run_poll(&host, "w3", "run-1", Some("0")).expect("polled");
        assert_eq!(polled["output"], "hi\n");
        station_run_stop(&host, "w3", "run-1").expect("stopped");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                Op::Run(RunOp::Start {
                    command: "ls".into(),
                    timeout_s: Some(30)
                }),
                Op::Run(RunOp::Poll {
                    run: "run-1".into(),
                    offset: Some(0)
                }),
                Op::Run(RunOp::Stop {
                    run: "run-1".into()
                }),
            ]
        );
        let rows = host
            .db
            .ide_audit(Some("station:w3"), 10)
            .expect("the audit reads");
        let kinds: Vec<&str> = rows.iter().filter_map(|row| row["kind"].as_str()).collect();
        assert_eq!(
            kinds,
            ["run_stop", "run"],
            "newest first, and a poll is not audited"
        );
        assert_eq!(
            host.db
                .station_audit(Some("w3"), 10)
                .expect("the station audit reads")
                .len(),
            2
        );
    }

    #[test]
    fn a_station_has_no_checkpoints_and_says_why() {
        let (link, _) = scripted_station(|_| Ok(Value::Null));
        let host =
            Fake::new("station-checkpoints", |_, _| Err("no tool".into())).with_station("w3", link);
        let answer = station_checkpoints(&host, "w3").expect("answers");
        assert_eq!(answer["checkpoints"], json!([]));
        assert!(
            answer["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("no checkpoints"))
        );
        assert_eq!(
            status_of(not_for_stations(NO_CHECKPOINTS)),
            501,
            "changes and restore are not served"
        );
    }

    #[test]
    fn a_station_is_refused_while_stop_all_holds_and_when_it_is_not_online() {
        let (link, seen) = scripted_station(|_| Ok(Value::Null));
        let frozen = Fake::new("station-frozen", |_, _| Err("no tool".into()))
            .with_station("w3", link.clone())
            .with_stop_all();
        assert_eq!(
            check_station(&frozen, "w3")
                .expect_err("STOP ALL refuses")
                .status,
            409
        );
        let online =
            Fake::new("station-online", |_, _| Err("no tool".into())).with_station("w3", link);
        assert!(check_station(&online, "w3").is_ok());
        assert_eq!(
            check_station(&online, "w9")
                .expect_err("not an online station")
                .status,
            404
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "no refused check reaches the daemon"
        );
    }

    #[test]
    fn a_station_answer_maps_to_a_status_by_why_it_failed() {
        assert_eq!(
            status_of::<Value>(Err(station_failure("frozen by STOP ALL".into()))),
            409
        );
        assert_eq!(
            status_of::<Value>(Err(station_failure("the workstation is not linked".into()))),
            409
        );
        assert_eq!(
            status_of::<Value>(Err(station_failure("the workstation is draining".into()))),
            409
        );
        assert_eq!(
            status_of::<Value>(Err(station_failure(
                "the workstation did not answer in time".into()
            ))),
            504
        );
        assert_eq!(
            status_of::<Value>(Err(station_failure("no such file".into()))),
            400
        );
    }
}
