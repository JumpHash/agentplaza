//! Per-agent permission modes and the owner's approvals queue.
//!
//! A mode says what an agent may do with the tools that change things.
//! `autonomous` (the default) lets them run, `ask` holds each call until the
//! owner approves that exact call, and `read_only` refuses them. Read-class
//! tools pass in every mode. Modes persist in `permissions.json`; the queue is
//! kept in memory (snapshotted to Postgres or the state dir with the other components), newest last, capped at `QUEUE_CAP`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_metrics::{EntityKind, SeriesKey};
use apz_proto::wire::HostMessage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::providers::write_atomic;
use crate::state::App;

/// How many approvals the queue keeps; the oldest drop first.
const QUEUE_CAP: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Autonomous,
    Ask,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Approved,
    Denied,
    Used,
    /// No longer needed: the plaza settled what it was about before the owner decided.
    Withdrawn,
}

/// An engine call the harness is waiting on for an owner's decision: the work item it runs under
/// and the call's id in the harness (`<call>.e<n>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub work: u64,
    pub call: String,
}

/// What a held call comes with: the thread of its work item, and, from a harness that can hold a
/// call open (an ACP engine's request), the call the harness is waiting on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    pub thread: Option<String>,
    pub waiting: Option<Waiting>,
}

/// The gate's answer for one call.
#[derive(Debug)]
pub enum Gate {
    /// The call may run.
    Run,
    /// The call is refused; the reason is what the agent sees.
    Refused(String),
    /// The call waits for the owner. `approval` is the item it waits on, `refusal` what the agent
    /// sees, and `created` the item this call queued (None when it joined one already pending).
    Held {
        approval: String,
        refusal: String,
        created: Option<Approval>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub cid: u64,
    pub tool: String,
    pub input: Value,
    pub created_ms: u64,
    pub state: State,
    pub reason: Option<String>,
    /// What the decision is: `tool` (a held tool call), `sanction`, `payment` or `review` (a done
    /// task no reviewer took in time; the approval passes it and the denial fails it).
    #[serde(default = "tool_kind")]
    pub kind: String,
    /// What the decision is about, in words. For a tool approval, the tool.
    #[serde(default)]
    pub subject: String,
    /// The structured detail. For a tool approval, the input.
    #[serde(default)]
    pub detail: Value,
    /// The thread of the work item the call ran under: the decision's follow-up is queued there.
    /// None when no work item was at hand, or for an approval recorded before this existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The engine calls waiting on this decision; each gets a `permission_decision` when the owner
    /// decides. Empty for every other approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<Waiting>,
}

fn tool_kind() -> String {
    "tool".to_string()
}

impl Approval {
    /// Fills the tool-approval fields a snapshot from before they existed lacks: the
    /// subject is the tool and the detail is the input. Other kinds are left as they are.
    fn backfill(&mut self) {
        if self.kind != "tool" {
            return;
        }
        if self.subject.is_empty() {
            self.subject = self.tool.clone();
        }
        if self.detail.is_null() {
            self.detail = self.input.clone();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecideError {
    NotFound,
    NotPending,
}

#[derive(Default)]
struct Inner {
    modes: BTreeMap<u64, Mode>,
    queue: Vec<Approval>,
    next: u64,
}

/// Modes (persisted) and the approvals queue (in memory).
pub struct Store {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl Store {
    /// Loads the saved modes from `dir`. A missing or unreadable file means
    /// every agent starts in `autonomous`.
    pub fn load(dir: &Path) -> Store {
        let path = dir.join("permissions.json");
        let modes = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<u64, Mode>>(&bytes).ok())
            .unwrap_or_default();
        Store {
            path,
            inner: Mutex::new(Inner {
                modes,
                queue: Vec::new(),
                next: 1,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn mode(&self, cid: u64) -> Mode {
        self.lock().modes.get(&cid).copied().unwrap_or_default()
    }

    /// Saves the mode for `cid` and writes the whole map to disk. On a failed
    /// write the in-memory mode is put back.
    pub fn set_mode(&self, cid: u64, mode: Mode) -> io::Result<()> {
        let mut inner = self.lock();
        let previous = inner.modes.insert(cid, mode);
        let saved = serde_json::to_vec_pretty(&inner.modes)
            .map_err(io::Error::other)
            .and_then(|bytes| write_atomic(&self.path, &bytes, 0o600));
        if saved.is_err() {
            match previous {
                Some(p) => {
                    inner.modes.insert(cid, p);
                }
                None => {
                    inner.modes.remove(&cid);
                }
            }
        }
        saved
    }

    /// The gate for one call. `class` is the tool's risk class. A held call is
    /// recorded with `held`: its thread, and the harness call waiting on the owner
    /// (a second identical call that joins a pending item adds its waiter to it).
    #[cfg(test)]
    pub fn gate(&self, cid: u64, tool: &str, class: &str, input: &Value, held: &Held) -> Gate {
        self.gate_with(cid, tool, class, input, held, Mode::default())
    }

    /// The mode the owner set for agent `cid`, if they set one.
    pub fn explicit(&self, cid: u64) -> Option<Mode> {
        self.lock().modes.get(&cid).copied()
    }

    /// [`Store::gate`], with `fallback` the mode of an agent the owner set none for (its project's).
    pub fn gate_with(
        &self,
        cid: u64,
        tool: &str,
        class: &str,
        input: &Value,
        held: &Held,
        fallback: Mode,
    ) -> Gate {
        if class == "read" {
            return Gate::Run;
        }
        let mut inner = self.lock();
        match inner.modes.get(&cid).copied().unwrap_or(fallback) {
            Mode::Autonomous => Gate::Run,
            Mode::ReadOnly => {
                Gate::Refused(format!("this agent is read-only: {tool} is not allowed"))
            }
            Mode::Ask => {
                if let Some(a) = inner.queue.iter_mut().find(|a| {
                    a.cid == cid
                        && a.tool == tool
                        && a.state == State::Approved
                        && a.input == *input
                }) {
                    a.state = State::Used;
                    return Gate::Run;
                }
                if let Some(a) = inner.queue.iter_mut().find(|a| {
                    a.cid == cid && a.tool == tool && a.state == State::Pending && a.input == *input
                }) {
                    if let Some(waiter) = &held.waiting {
                        a.waiting.push(waiter.clone());
                    }
                    let approval = a.id.clone();
                    return Gate::Held {
                        refusal: format!("still waiting for the owner's approval ({approval})"),
                        approval,
                        created: None,
                    };
                }
                let mut item = push_pending(&mut inner, cid, "tool", tool, input.clone());
                item.thread = held.thread.clone();
                item.waiting = held.waiting.iter().cloned().collect();
                // push_pending appends, so the item just queued is the last one.
                if let Some(stored) = inner.queue.last_mut() {
                    stored.thread = item.thread.clone();
                    stored.waiting = item.waiting.clone();
                }
                let refusal = format!(
                    "waiting for the owner's approval ({}); you'll be told when it is decided, then run the same call again",
                    item.id
                );
                Gate::Held {
                    approval: item.id.clone(),
                    refusal,
                    created: Some(item),
                }
            }
        }
    }

    /// Queues a pending decision of `kind` about `subject` for agent `cid`, in any mode, and
    /// returns it. The queue cap applies.
    pub fn submit(&self, cid: u64, kind: &str, subject: &str, detail: Value) -> Approval {
        let mut inner = self.lock();
        push_pending(&mut inner, cid, kind, subject, detail)
    }

    /// Records the owner's decision on a pending approval.
    pub fn decide(
        &self,
        id: &str,
        approve: bool,
        reason: Option<String>,
    ) -> Result<Approval, DecideError> {
        let mut inner = self.lock();
        let item = inner
            .queue
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(DecideError::NotFound)?;
        if item.state != State::Pending {
            return Err(DecideError::NotPending);
        }
        item.state = if approve {
            State::Approved
        } else {
            State::Denied
        };
        item.reason = reason.filter(|r| !r.trim().is_empty());
        Ok(item.clone())
    }

    /// Withdraws the pending approval `id` with `reason`, so the owner is no longer asked. Only a
    /// pending approval can be withdrawn; anything else is left as it is and None is returned.
    pub fn withdraw(&self, id: &str, reason: &str) -> Option<Approval> {
        let mut inner = self.lock();
        let item = inner
            .queue
            .iter_mut()
            .find(|a| a.id == id && a.state == State::Pending)?;
        item.state = State::Withdrawn;
        item.reason = Some(reason.to_string());
        Some(item.clone())
    }

    /// The queue, newest first.
    pub fn list(&self) -> Vec<Approval> {
        self.lock().queue.iter().rev().cloned().collect()
    }

    /// How many approvals wait on the owner now.
    pub fn pending_count(&self) -> usize {
        self.lock()
            .queue
            .iter()
            .filter(|a| a.state == State::Pending)
            .count()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Appends a pending item to the queue under the next id, drops the oldest past
/// `QUEUE_CAP`, and returns the item. `tool` and `subject` are the same string, and
/// `input` and `detail` are the same value, so a tool approval and a decision of
/// another kind read alike.
fn push_pending(inner: &mut Inner, cid: u64, kind: &str, subject: &str, detail: Value) -> Approval {
    let id = format!("a{}", inner.next);
    inner.next += 1;
    let item = Approval {
        id,
        cid,
        tool: subject.to_string(),
        input: detail.clone(),
        created_ms: now_ms(),
        state: State::Pending,
        reason: None,
        kind: kind.to_string(),
        subject: subject.to_string(),
        detail,
        thread: None,
        waiting: Vec::new(),
    };
    inner.queue.push(item.clone());
    if inner.queue.len() > QUEUE_CAP {
        let excess = inner.queue.len() - QUEUE_CAP;
        inner.queue.drain(..excess);
    }
    item
}

/// Called on the tool path before a call runs. Publishes a newly queued approval and
/// returns the gate's answer.
pub fn gate(app: &App, cid: u64, tool: &str, input: &Value, held: &Held) -> Gate {
    // An agent the owner set no mode for has its project's.
    let fallback = crate::project_settings::permission_mode(app, cid);
    let gate = app.approvals.gate_with(
        cid,
        tool,
        crate::laws::risk_class(tool),
        input,
        held,
        fallback,
    );
    if let Gate::Held {
        created: Some(item),
        ..
    } = &gate
    {
        app.bus.publish("approval", item);
        record_pushed(app, cid);
    }
    gate
}

/// Queues a decision of `kind` about `subject` for the owner, whatever the agent's mode,
/// publishes it, and returns it. The agent is `detail["cid"]`, or 0 when there is none.
// Used by the sanctions and payments work, which is not wired yet.
#[allow(dead_code)]
pub fn submit(app: &App, kind: &str, subject: &str, detail: Value) -> Approval {
    let cid = detail_cid(&detail);
    let item = app.approvals.submit(cid, kind, subject, detail);
    app.bus.publish("approval", &item);
    record_pushed(app, cid);
    item
}

/// Records a newly queued approval for agent `cid`: the waiting count and the agent's state.
fn record_pushed(app: &App, cid: u64) {
    record_pending(app);
    crate::metrics::collect::agent_state(app, cid, "approval");
}

/// Sets the `approval.pending` gauge to the approvals waiting on the owner now.
fn record_pending(app: &App) {
    app.metrics
        .registry
        .gauge(SeriesKey::new(
            "approval.pending",
            EntityKind::Global,
            "global",
        ))
        .set(app.approvals.pending_count() as f64);
}

/// The agent a decision's detail names (`cid`), or 0 when it names none.
#[allow(dead_code)]
fn detail_cid(detail: &Value) -> u64 {
    detail.get("cid").and_then(Value::as_u64).unwrap_or(0)
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/approvals", list)
        .doc(RouteDoc::new("List the approvals, pending and decided"))
        .post("/v1/approvals/{id}", decide)
        .doc(RouteDoc::new("Approve or reject a pending approval").errors(&[400, 404, 409]))
        .get("/v1/agents/{cid}/permissions", get_mode)
        .doc(RouteDoc::new("Show an agent's permission mode"))
        .route("PUT", "/v1/agents/{cid}/permissions", put_mode)
        .doc(RouteDoc::new("Set an agent's permission mode").errors(&[400, 500]))
}

#[derive(Deserialize)]
struct DecideBody {
    approve: bool,
    #[serde(default)]
    reason: Option<String>,
    /// For a sanction decision: `acknowledge`, `lift` or `retire` (see `sanctions::decided`).
    #[serde(default)]
    action: Option<String>,
}

#[derive(Deserialize)]
struct ModeBody {
    mode: Mode,
}

fn list(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    Response::json(200, &app.approvals.list())
}

fn decide(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let body: DecideBody = match req.json() {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                400,
                "body must be {\"approve\": bool, \"reason\": string?, \"action\": string?}",
            );
        }
    };
    match decide_approval(app, id, body.approve, body.reason, body.action.as_deref()) {
        Ok(item) => Response::json(200, &item),
        Err((code, message)) => Response::error(code, &message),
    }
}

/// Records the owner's decision on approval `id`, then does what its kind needs. The approvals API and
/// the editors' ACP both decide this way. The error is the status and message for the caller.
pub(crate) fn decide_approval(
    app: &App,
    id: &str,
    approve: bool,
    reason: Option<String>,
    action: Option<&str>,
) -> Result<Value, (u16, String)> {
    let item = match app.approvals.decide(id, approve, reason) {
        Ok(item) => item,
        Err(DecideError::NotFound) => return Err((404, "no such approval".to_string())),
        Err(DecideError::NotPending) => {
            return Err((409, "this approval is already decided".to_string()));
        }
    };
    app.metrics
        .registry
        .hist(SeriesKey::new(
            "approval.wait_ms",
            EntityKind::Agent,
            &item.cid.to_string(),
        ))
        .record(now_ms().saturating_sub(item.created_ms) as f64);
    record_pending(app);
    app.bus.publish("approval", &item);
    // The decision is recorded; each kind then does its own work with it.
    let applied = match item.kind.as_str() {
        "sanction" => crate::sanctions::decided(app, &item, approve, action),
        "payment" => crate::payments::decided(app, &item, approve),
        "review" => crate::review_graph::owner_ruled(app, &item, approve, action),
        "deploy" => crate::environments::decided(app, &item, approve),
        "artifact.share" => crate::shares::request::on_decided(app, &item),
        _ => {
            // A held engine call gets the decision pushed to it. The agent is told in chat only when
            // no call is waiting, or the push did not reach it (the decision must not be lost).
            if item.waiting.is_empty()
                || push_decisions(&item, |cid, line| send_to_agent(app, cid, line)).is_err()
            {
                tell_agent(app, &item);
            }
            Ok(())
        }
    };
    match applied {
        Ok(()) => Ok(serde_json::to_value(&item).unwrap_or(Value::Null)),
        Err(e) => Err((409, e)),
    }
}

/// Sends each engine call waiting on `item` its `permission_decision`: the owner's verdict and note,
/// pushed to the agent that runs the call. `send` is the gateway path in production. Stops at the
/// first failure and returns it, so the caller can tell the agent another way.
pub(crate) fn push_decisions(
    item: &Approval,
    send: impl Fn(u64, &HostMessage) -> Result<(), String>,
) -> Result<(), String> {
    let allow = item.state == State::Approved;
    for waiter in &item.waiting {
        let line = HostMessage::PermissionDecision {
            work: waiter.work,
            call: waiter.call.clone(),
            allow,
            reason: item.reason.clone(),
        };
        send(item.cid, &line)?;
    }
    Ok(())
}

/// One line to agent `cid` through its gateway (`/v1/agents/{cid}/send`).
fn send_to_agent(app: &App, cid: u64, line: &HostMessage) -> Result<(), String> {
    match crate::fleet::send(app, cid, &json!({ "message": line })) {
        Ok((code, _)) if (200..300).contains(&code) => Ok(()),
        Ok((code, _)) => Err(format!("the agent's gateway answered {code}")),
        Err(err) => Err(err),
    }
}

/// The thread a decision's follow-up is queued on: the work item's own thread, so the agent hears
/// it where the call was made. `op-<cid>` when none was recorded.
pub(crate) fn tell_thread(item: &Approval) -> String {
    item.thread
        .clone()
        .unwrap_or_else(|| format!("op-{}", item.cid))
}

/// Tells the agent a tool approval was decided, as a chat message.
fn tell_agent(app: &App, item: &Approval) {
    let approved = item.state == State::Approved;
    let verb = if approved { "approved" } else { "denied" };
    let why = match &item.reason {
        Some(r) => format!(": {r}"),
        None => String::new(),
    };
    let tail = if approved {
        " Run the same call again now."
    } else {
        ""
    };
    let text = format!(
        "[plaza] The owner {verb} your {} call ({}){why}.{tail}",
        item.tool, item.id
    );
    app.work.enqueue(
        app,
        item.cid,
        apz_proto::wire::WorkKind::Chat,
        tell_thread(item),
        text,
        crate::scheduler::Origin::default(),
    );
}

fn cid_param(p: &Params<'_>) -> Option<u64> {
    p.get("cid").parse().ok()
}

fn get_mode(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let Some(cid) = cid_param(p) else {
        return Response::error(400, "cid must be a number");
    };
    // The mode in force, and whether the owner set it for this agent or it is its project's (or the default).
    let (mode, source) = match app.approvals.explicit(cid) {
        Some(mode) => (mode, "agent"),
        None => match crate::project_settings::of_agent(app, cid).and_then(|s| s.permission_mode) {
            Some(mode) => (mode, "project"),
            None => (Mode::default(), "default"),
        },
    };
    Response::json(200, &json!({ "mode": mode, "source": source }))
}

fn put_mode(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Some(cid) = cid_param(p) else {
        return Response::error(400, "cid must be a number");
    };
    let body: ModeBody = match req.json() {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                400,
                "body must be {\"mode\": \"autonomous\" | \"ask\" | \"read_only\"}",
            );
        }
    };
    // A mode is set for an agent on the roster only: an unknown CID would keep a mode no agent uses.
    if app.agents.get(cid).is_none() {
        return Response::error(404, "no such agent");
    }
    if let Err(e) = app.approvals.set_mode(cid, body.mode) {
        return Response::error(500, &format!("could not save the permission mode: {e}"));
    }
    Response::json(200, &json!({ "mode": body.mode }))
}

/// The snapshot's shape: the modes, the queue (`pending`, every item whatever its state) and the next id.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    modes: BTreeMap<u64, Mode>,
    pending: Vec<Approval>,
    #[serde(default)]
    seq: u64,
}

impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        let snap = Snapshot {
            modes: inner.modes.clone(),
            pending: inner.queue.clone(),
            seq: inner.next,
        };
        serde_json::to_string(&snap).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let snap: Snapshot =
            serde_json::from_str(json).map_err(|e| format!("bad approvals snapshot: {e}"))?;
        let bytes = serde_json::to_vec_pretty(&snap.modes).map_err(|e| e.to_string())?;
        let mut pending = snap.pending;
        for item in &mut pending {
            item.backfill();
        }
        let excess = pending.len().saturating_sub(QUEUE_CAP);
        pending.drain(..excess);
        let mut inner = self.lock();
        write_atomic(&self.path, &bytes, 0o600).map_err(|e| format!("write permissions: {e}"))?;
        inner.modes = snap.modes;
        inner.queue = pending;
        inner.next = snap.seq.max(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Store {
        /// The gate as the chat-path tests read it: the refusal the agent sees, and the item queued.
        fn gate_plain(
            &self,
            cid: u64,
            tool: &str,
            class: &str,
            input: &Value,
        ) -> (Option<String>, Option<Approval>) {
            match self.gate(cid, tool, class, input, &Held::default()) {
                Gate::Run => (None, None),
                Gate::Refused(reason) => (Some(reason), None),
                Gate::Held {
                    refusal, created, ..
                } => (Some(refusal), created),
            }
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-approvals-{tag}-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn mode_round_trips_through_disk() {
        let dir = scratch("mode");
        let store = Store::load(&dir);
        assert_eq!(store.mode(7), Mode::Autonomous);
        store.set_mode(7, Mode::ReadOnly).unwrap();
        store.set_mode(9, Mode::Ask).unwrap();

        let again = Store::load(&dir);
        assert_eq!(again.mode(7), Mode::ReadOnly);
        assert_eq!(again.mode(9), Mode::Ask);
        assert_eq!(again.mode(3), Mode::Autonomous);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ask_holds_a_call_until_approved_then_passes_once() {
        let dir = scratch("ask");
        let store = Store::load(&dir);
        store.set_mode(1, Mode::Ask).unwrap();
        let input = json!({ "path": "/x", "body": "y" });

        let (held, created) = store.gate_plain(1, "write_thing", "write", &input);
        let item = created.expect("a pending approval is queued");
        assert_eq!(item.state, State::Pending);
        assert!(held.unwrap().contains(&item.id));

        // The same call while still pending is held again, with no new item.
        let (again, none) = store.gate_plain(1, "write_thing", "write", &input);
        assert!(again.unwrap().starts_with("still waiting"));
        assert!(none.is_none());

        let approved = store.decide(&item.id, true, Some("ok".into())).unwrap();
        assert_eq!(approved.state, State::Approved);
        assert!(matches!(
            store.decide(&item.id, false, None),
            Err(DecideError::NotPending)
        ));

        // The matching call passes exactly once.
        assert_eq!(store.gate_plain(1, "write_thing", "write", &input).0, None);
        let used = store.list().into_iter().find(|a| a.id == item.id).unwrap();
        assert_eq!(used.state, State::Used);
        assert!(
            store
                .gate_plain(1, "write_thing", "write", &input)
                .0
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_refuses_writes() {
        let dir = scratch("ro");
        let store = Store::load(&dir);
        store.set_mode(2, Mode::ReadOnly).unwrap();
        let (refusal, created) = store.gate_plain(2, "write_thing", "write", &json!({}));
        assert_eq!(
            refusal.unwrap(),
            "this agent is read-only: write_thing is not allowed"
        );
        assert!(created.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_pass_in_every_mode_and_autonomous_lets_writes_run() {
        let dir = scratch("read");
        let store = Store::load(&dir);
        for (cid, mode) in [(1, Mode::Autonomous), (2, Mode::Ask), (3, Mode::ReadOnly)] {
            store.set_mode(cid, mode).unwrap();
            let (refusal, created) = store.gate_plain(cid, "read_thing", "read", &json!({}));
            assert!(
                refusal.is_none() && created.is_none(),
                "{mode:?} must pass reads"
            );
        }
        assert!(store.list().is_empty());
        // An autonomous agent's write passes straight through, nothing queued.
        let (refusal, created) = store.gate_plain(1, "write_thing", "write", &json!({}));
        assert!(
            refusal.is_none() && created.is_none(),
            "autonomous writes run"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_round_trips_modes_and_queue() {
        let dir = scratch("snap");
        let store = Store::load(&dir);
        store.set_mode(5, Mode::Ask).unwrap();
        let (_, created) = store.gate_plain(5, "write_thing", "write", &json!({ "a": 1 }));
        let id = created.unwrap().id;

        let other_dir = scratch("snap-other");
        let other = Store::load(&other_dir);
        other.restore(&store.snapshot()).unwrap();
        assert_eq!(other.mode(5), Mode::Ask);
        assert_eq!(other.list().len(), 1);
        assert_eq!(other.list()[0].id, id);
        assert_eq!(Store::load(&other_dir).mode(5), Mode::Ask);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other_dir);
    }

    #[test]
    fn submit_queues_a_pending_decision_in_any_mode() {
        let dir = scratch("submit");
        let store = Store::load(&dir);
        store.set_mode(6, Mode::ReadOnly).unwrap();
        let detail = json!({ "cid": 6, "amount": 900, "to": "bob" });
        let item = store.submit(6, "payment", "pay 900 to bob", detail.clone());
        assert_eq!(item.state, State::Pending);
        assert_eq!(item.kind, "payment");
        assert_eq!(item.subject, "pay 900 to bob");
        assert_eq!(item.tool, "pay 900 to bob");
        assert_eq!(item.detail, detail);
        assert_eq!(item.input, detail);
        assert_eq!(item.cid, 6);
        assert_eq!(store.list()[0].id, item.id);
        // Gate and submit share one id counter: the held call gets the next id.
        store.set_mode(7, Mode::Ask).unwrap();
        let (_, held) = store.gate_plain(7, "write_thing", "write", &json!({}));
        let held = held.expect("a held call queues an item");
        let next: u64 = item.id[1..].parse::<u64>().unwrap() + 1;
        assert_eq!(held.id, format!("a{next}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_snapshot_without_the_new_fields_restores_as_a_tool_approval() {
        let old = r#"{"modes":{"5":"ask"},"pending":[{"id":"a3","cid":5,"tool":"write_thing","input":{"a":1},"created_ms":10,"state":"pending","reason":null}],"seq":4}"#;
        let store = Store::load(&scratch("old"));
        store.restore(old).unwrap();
        assert_eq!(store.mode(5), Mode::Ask);
        let item = store.list().into_iter().next().unwrap();
        assert_eq!(item.kind, "tool");
        assert_eq!(item.subject, "write_thing");
        assert_eq!(item.detail, json!({ "a": 1 }));
        // The snapshot it writes now carries the new fields, and reads back.
        let other = Store::load(&scratch("old-other"));
        other.restore(&store.snapshot()).unwrap();
        assert_eq!(other.list()[0].kind, "tool");
    }

    #[test]
    fn submit_holds_the_queue_cap() {
        let store = Store::load(&scratch("cap"));
        for i in 0..(QUEUE_CAP + 5) {
            store.submit(1, "payment", &format!("pay {i}"), json!({ "cid": 1 }));
        }
        let list = store.list();
        assert_eq!(list.len(), QUEUE_CAP);
        // Newest first: the five oldest were dropped.
        assert_eq!(list[0].subject, format!("pay {}", QUEUE_CAP + 4));
        assert_eq!(list[QUEUE_CAP - 1].subject, "pay 5");
    }

    #[test]
    fn a_held_engine_call_keeps_its_thread_and_waiter_and_joins_a_pending_item() {
        let dir = scratch("held");
        let store = Store::load(&dir);
        store.set_mode(8, Mode::Ask).unwrap();
        let input = json!({ "engine": "claude", "title": "edit" });
        let first = Held {
            thread: Some("acp-s1".into()),
            waiting: Some(Waiting {
                work: 3,
                call: "toolu_1.e1".into(),
            }),
        };
        let Gate::Held {
            approval,
            created: Some(item),
            ..
        } = store.gate(8, "engine.edit", "write", &input, &first)
        else {
            panic!("the first call is held and queued");
        };
        assert_eq!(item.id, approval);
        assert_eq!(item.thread.as_deref(), Some("acp-s1"));
        assert_eq!(
            item.waiting,
            first.waiting.iter().cloned().collect::<Vec<_>>()
        );

        // The same call from another engine request joins the pending item: no new item.
        let second = Held {
            thread: Some("acp-s1".into()),
            waiting: Some(Waiting {
                work: 3,
                call: "toolu_1.e2".into(),
            }),
        };
        match store.gate(8, "engine.edit", "write", &input, &second) {
            Gate::Held {
                approval: again,
                created: None,
                ..
            } => assert_eq!(again, approval),
            other => panic!("expected a join, got {other:?}"),
        }
        let stored = store.list().into_iter().find(|a| a.id == approval).unwrap();
        assert_eq!(stored.waiting.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_decision_is_pushed_to_each_waiter_and_a_failed_push_is_reported() {
        let dir = scratch("push");
        let store = Store::load(&dir);
        store.set_mode(4, Mode::Ask).unwrap();
        let input = json!({ "path": "/w/x" });
        let held = Held {
            thread: None,
            waiting: Some(Waiting {
                work: 9,
                call: "toolu_7.e1".into(),
            }),
        };
        let Gate::Held { approval, .. } = store.gate(4, "engine.edit", "write", &input, &held)
        else {
            panic!("held");
        };
        let joined = Held {
            thread: None,
            waiting: Some(Waiting {
                work: 9,
                call: "toolu_7.e2".into(),
            }),
        };
        store.gate(4, "engine.edit", "write", &input, &joined);
        let item = store
            .decide(&approval, true, Some("go ahead".into()))
            .unwrap();

        let sent = std::cell::RefCell::new(Vec::new());
        push_decisions(&item, |cid, line| {
            sent.borrow_mut().push((cid, line.clone()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            sent.into_inner(),
            vec![
                (
                    4,
                    HostMessage::PermissionDecision {
                        work: 9,
                        call: "toolu_7.e1".into(),
                        allow: true,
                        reason: Some("go ahead".into()),
                    }
                ),
                (
                    4,
                    HostMessage::PermissionDecision {
                        work: 9,
                        call: "toolu_7.e2".into(),
                        allow: true,
                        reason: Some("go ahead".into()),
                    }
                ),
            ]
        );

        // A refused push stops at the failure, which the caller sees.
        let refused = push_decisions(&item, |_, _| Err("the gateway answered 409".into()));
        assert_eq!(refused, Err("the gateway answered 409".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_denial_is_pushed_as_a_denial() {
        let item = Approval {
            id: "a1".into(),
            cid: 2,
            tool: "engine.edit".into(),
            input: json!({}),
            created_ms: 0,
            state: State::Denied,
            reason: None,
            kind: "tool".into(),
            subject: "engine.edit".into(),
            detail: json!({}),
            thread: None,
            waiting: vec![Waiting {
                work: 1,
                call: "toolu_2.e1".into(),
            }],
        };
        let sent = std::cell::RefCell::new(Vec::new());
        push_decisions(&item, |_, line| {
            sent.borrow_mut().push(line.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            sent.into_inner(),
            vec![HostMessage::PermissionDecision {
                work: 1,
                call: "toolu_2.e1".into(),
                allow: false,
                reason: None,
            }]
        );
    }

    #[test]
    fn the_follow_up_goes_to_the_work_thread_or_to_op_cid() {
        let mut item = Approval {
            id: "a2".into(),
            cid: 5,
            tool: "write_thing".into(),
            input: json!({}),
            created_ms: 0,
            state: State::Approved,
            reason: None,
            kind: "tool".into(),
            subject: "write_thing".into(),
            detail: json!({}),
            thread: None,
            waiting: Vec::new(),
        };
        assert_eq!(tell_thread(&item), "op-5");
        item.thread = Some("acp-s9".into());
        assert_eq!(tell_thread(&item), "acp-s9");
    }
}
