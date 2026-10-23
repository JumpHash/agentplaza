//! Per-agent permission modes and the owner's approvals queue.
//!
//! A mode says what an agent may do with the tools that change things.
//! `autonomous` (the default) lets them run, `ask` holds each call until the
//! owner approves that exact call, and `read_only` refuses them. Read-class
//! tools pass in every mode. Modes and the queue (newest last, capped at `QUEUE_CAP`) are kept in the hub's store
//! (`approval_queue`: the modes as a map, the queue and its id counter as child rows); the owner's decisions and mode
//! changes are saved at once (`saver::save_now`).
//!
//! If the saved state cannot be read at start, every agent without a mode runs in `ask`, and nothing is written over
//! the stored state until the hub restarts with a readable one.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_metrics::{EntityKind, SeriesKey};
use apz_proto::wire::HostMessage;
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cards::{self, Card, Choice, NewCard};
use crate::state::App;

/// How many approvals the queue keeps; the oldest drop first.
const QUEUE_CAP: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Autonomous,
    Ask,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
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

/// One approval: a row of the approvals queue (`approval_queue_items`). Its call's input and its detail are kept as
/// written (content); the hub matches a held call against an approved one by its input.
#[derive(Debug, Clone, Serialize, Deserialize, apz_store::Table)]
pub struct Approval {
    pub id: String,
    pub cid: u64,
    pub tool: String,
    /// The tool call's input (JSON), as the model sent it.
    #[table(content)]
    pub input: Value,
    pub created_ms: u64,
    pub state: State,
    pub reason: Option<String>,
    /// What the decision is: `tool` (a held tool call) or `review` (a done
    /// task no reviewer took in time; the approval passes it and the denial fails it).
    #[serde(default = "tool_kind")]
    pub kind: String,
    /// What the decision is about, in words. For a tool approval, the tool.
    #[serde(default)]
    pub subject: String,
    /// The structured detail. For a tool approval, the input.
    #[serde(default)]
    #[table(content)]
    pub detail: Value,
    /// The thread of the work item the call ran under: the decision's follow-up is queued there.
    /// None when no work item was at hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The engine calls waiting on this decision; each gets a `permission_decision` when the owner
    /// decides. Empty for every other approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(child)]
    pub waiting: Vec<Waiting>,
}

fn tool_kind() -> String {
    "tool".to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecideError {
    NotFound,
    NotPending,
}

/// The store's state in memory: the modes, the queue (every item whatever its state, oldest first), the id counter,
/// and whether the saved state could not be read at start.
#[derive(Default)]
struct Inner {
    modes: BTreeMap<u64, Mode>,
    queue: Vec<Approval>,
    /// The last id handed out (`a<next>`).
    next: u64,
    /// Set when the saved state could not be read at start: the modes are then unknown, so an agent with no mode of
    /// its own runs in `ask`, and nothing is written over the stored state until the hub restarts.
    damaged: bool,
}

/// The approvals' state as its table: one row of `approval_queue`, the modes a child map and the queue a child list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, apz_store::Table)]
#[table(name = "approval_queue")]
struct Queue {
    #[table(map_value)]
    modes: BTreeMap<u64, Mode>,
    #[table(child)]
    items: Vec<Approval>,
    /// The last id handed out.
    seq: u64,
}

/// The approvals' table: `approval_queue`.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Queue)
}

/// Modes and the approvals queue, in memory: the hub's saver keeps them in `plaza.db`.
#[derive(Default)]
pub struct Store {
    inner: Mutex<Inner>,
}

impl Store {
    /// Replaces the modes and the queue with what the hub's store holds (at start). A store that cannot be read is
    /// marked damaged: every agent without a mode of its own runs in `ask`, and nothing is written over the stored
    /// state until the hub restarts with a readable one.
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = match apz_store::table::load_one::<Queue>(conn, def()) {
            Ok(stored) => stored.unwrap_or_default(),
            Err(e) => {
                self.lock().damaged = true;
                return Err(e);
            }
        };
        let mut items = stored.items;
        let excess = items.len().saturating_sub(QUEUE_CAP);
        items.drain(..excess);
        let mut inner = self.lock();
        inner.modes = stored.modes;
        inner.queue = items;
        inner.next = stored.seq;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The modes and the queue as their rows, as they are now (the saver writes what changed). Nothing while the saved
    /// state could not be read: an empty row set writes nothing, so nothing is written over the stored state.
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let inner = self.lock();
        if inner.damaged {
            return Ok(RowSet::new());
        }
        let queue = Queue {
            modes: inner.modes.clone(),
            items: inner.queue.clone(),
            seq: inner.next,
        };
        RowSet::one(def(), &queue)
    }

    /// Whether the saved state could not be read at start: the modes then apply in memory only.
    pub fn damaged(&self) -> bool {
        self.lock().damaged
    }

    /// The mode of agent `cid` when the owner set none: `ask` while the saved state is unreadable, else `autonomous`.
    fn fallback(inner: &Inner) -> Mode {
        if inner.damaged {
            Mode::Ask
        } else {
            Mode::default()
        }
    }

    pub fn mode(&self, cid: u64) -> Mode {
        let inner = self.lock();
        inner
            .modes
            .get(&cid)
            .copied()
            .unwrap_or_else(|| Self::fallback(&inner))
    }

    /// Sets the mode for `cid`. The saver writes it (at once for the owner's change: `put_mode`). While the saved state
    /// is unreadable the mode applies in memory only.
    pub fn set_mode(&self, cid: u64, mode: Mode) {
        let mut inner = self.lock();
        if inner.damaged {
            eprintln!(
                "approvals: agent {cid} is set to {mode:?} for this run only, not saved: the saved modes could not be read at startup and are not written until the hub restarts"
            );
        }
        inner.modes.insert(cid, mode);
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

    /// The mode the gate enforces for agent `cid` whose project's mode is `project` (None when the
    /// project sets none), and where it comes from: the owner's (`agent`), the project's (`project`),
    /// the default (`default`), or `damaged-file-fallback` (the label the console reads) when the saved modes could not
    /// be read and the agent's mode is `ask`.
    pub fn effective(&self, cid: u64, project: Option<Mode>) -> (Mode, &'static str) {
        if let Some(mode) = self.explicit(cid) {
            return (mode, "agent");
        }
        match (self.damaged(), project) {
            (true, None | Some(Mode::Autonomous)) => (Mode::Ask, "damaged-file-fallback"),
            (_, Some(mode)) => (mode, "project"),
            (_, None) => (Mode::default(), "default"),
        }
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
        // Unreadable saved modes make `ask` the default of an agent with no saved mode, even when
        // its project's mode is the default `autonomous`.
        let default = match (inner.damaged, fallback) {
            (true, Mode::Autonomous) => Mode::Ask,
            (_, own) => own,
        };
        match inner.modes.get(&cid).copied().unwrap_or(default) {
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
    inner.next += 1;
    let id = format!("a{}", inner.next);
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
        announce(app, item);
        record_pushed(app, cid);
    }
    gate
}

/// Queues a decision of `kind` about `subject` for the owner, whatever the agent's mode,
/// publishes it, and returns it. The agent is `detail["cid"]`, or 0 when there is none.
pub fn submit(app: &App, kind: &str, subject: &str, detail: Value) -> Approval {
    let cid = detail_cid(&detail);
    let item = app.approvals.submit(cid, kind, subject, detail);
    announce(app, &item);
    record_pushed(app, cid);
    item
}

/// The card an approval of `kind` gets, when its kind has one: a held tool call's (`tool_call`) and a review ruling's
/// (`review`). The other kinds (payments, deploys, shares) are decided in the console's queue alone.
fn card_kind(kind: &str) -> Option<cards::Kind> {
    match kind {
        "tool" => Some(cards::Kind::ToolCall),
        "review" => Some(cards::Kind::Review),
        _ => None,
    }
}

/// The subject of an approval's card.
fn card_subject(id: &str) -> String {
    format!("approval:{id}")
}

/// Publishes a new approval and opens its card in the chat it belongs to, when its kind has one.
pub fn announce(app: &App, item: &Approval) {
    app.bus.publish("approval", item);
    let now = now_ms();
    let card = match card_kind(&item.kind) {
        Some(cards::Kind::ToolCall) => tool_placed(app, item, now),
        Some(cards::Kind::Review) => review_placed(app, item, now),
        _ => return,
    };
    crate::work_loop::open_card(app, card, now);
}

/// The card a held tool call asks, shown in its session's task chat, else its project's, else the main chat.
fn tool_placed(app: &App, item: &Approval, now: u64) -> NewCard {
    let agent = app
        .agents
        .get(item.cid)
        .map_or_else(|| format!("Agent {}", item.cid), |a| a.label());
    let mut card = tool_content(item, &agent);
    if let Some(session) = app.sessions.by_cid(item.cid) {
        card.session = Some(session.id.clone());
        if let Some(task) = &session.task {
            card.chat = Some(crate::work_loop::task_chat(
                app,
                task,
                &session.project,
                now,
            ));
            card.project = Some(session.project.clone());
            card.task = Some(task.clone());
            return card;
        }
        if session.project != crate::work_loop::PLAZA {
            card.chat = Some(crate::work_loop::project_chat(app, &session.project, now));
            card.project = Some(session.project.clone());
            return card;
        }
    }
    card.chat = Some(app.chats.main(now).id);
    card
}

/// The card a review ruling asks, shown in its task's chat.
fn review_placed(app: &App, item: &Approval, now: u64) -> NewCard {
    let mut card = review_content(item);
    let task = card.task.clone().unwrap_or_default();
    match app
        .plaza
        .tasks
        .find(&task)
        .and_then(|t| t.project().map(str::to_string))
    {
        Some(project) => {
            card.chat = Some(crate::work_loop::task_chat(app, &task, &project, now));
            card.project = Some(project);
        }
        None => card.chat = Some(app.chats.main(now).id),
    }
    card
}

/// What a held tool call's card says, before it is placed: approve it (it runs once, as asked) or deny it.
fn tool_content(item: &Approval, agent: &str) -> NewCard {
    NewCard {
        kind: cards::Kind::ToolCall,
        chat: None,
        project: None,
        task: None,
        session: None,
        subject: card_subject(&item.id),
        title: format!("{agent} asks to run {}", item.tool),
        body: Some(format!(
            "Its permission mode is ask, so the call waits for you. Input: {}",
            clip(&item.input.to_string(), 300)
        )),
        choices: vec![
            Choice {
                id: "approve".into(),
                label: "Approve".into(),
                detail: Some("the call runs once, as it was asked".into()),
                takes_amount: false,
            },
            Choice {
                id: "deny".into(),
                label: "Deny".into(),
                detail: Some("the agent is told, and the call does not run".into()),
                takes_amount: false,
            },
        ],
    }
}

/// What a review ruling's card says, before it is placed: pass the task, send it back to its builder (only a review
/// run can take it back), or fail it.
fn review_content(item: &Approval) -> NewCard {
    let task = item
        .detail
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let text = ["verdict", "needs_you", "summary"]
        .iter()
        .filter_map(|key| item.detail.get(*key).and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let choice = |id: &str, label: &str, detail: &str| Choice {
        id: id.into(),
        label: label.into(),
        detail: Some(detail.into()),
        takes_amount: false,
    };
    let mut choices = vec![choice("pass", "Pass", "the work is accepted")];
    if item.detail.get("run").is_some() {
        choices.push(choice(
            "rework",
            "Send back",
            "to its builder, with your notes",
        ));
    }
    choices.push(choice(
        "fail",
        "Fail",
        "the task fails; the planner decides what follows",
    ));
    NewCard {
        kind: cards::Kind::Review,
        chat: None,
        project: None,
        task: Some(task.clone()),
        session: None,
        subject: card_subject(&item.id),
        title: format!("Task {task}'s review needs your ruling"),
        body: (!text.is_empty()).then(|| clip(&text, 400)),
        choices,
    }
}

/// `text` cut to `max` characters, with an ellipsis when it was cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// What a card's choice rules on its approval: whether it is approved, and the action a review ruling names. None for a
/// choice the card does not offer.
pub fn ruling(kind: cards::Kind, choice: &str) -> Option<(bool, Option<&'static str>)> {
    match (kind, choice) {
        (cards::Kind::ToolCall, "approve") | (cards::Kind::Review, "pass") => Some((true, None)),
        (cards::Kind::ToolCall, "deny") | (cards::Kind::Review, "fail") => Some((false, None)),
        (cards::Kind::Review, "rework") => Some((false, Some("rework"))),
        _ => None,
    }
}

/// Decides the approval a card asks about, as the card's choice rules: the same decision `POST /v1/approvals/{id}`
/// makes. Returns the answer in words.
pub fn card_answered(app: &App, card: &Card) -> String {
    let choice = card.answer.as_deref().unwrap_or_default();
    let (Some(id), Some((approve, action))) = (
        card.subject.strip_prefix("approval:"),
        ruling(card.kind, choice),
    ) else {
        return "recorded".into();
    };
    match decide_approval(app, id, approve, None, action) {
        Ok(_) => format!("{choice}: approval {id} is decided"),
        Err((_, why)) => why,
    }
}

/// Withdraws the open card of each approval in `items`, which were decided or withdrawn elsewhere: the owner is not
/// asked what is settled.
pub fn withdraw_cards(app: &App, items: &[Approval]) {
    for card in withdrawn_cards(&app.cards, items, now_ms()) {
        app.bus.publish("card", &card);
    }
}

/// The open cards of `items`' approvals, withdrawn; the cards withdrawn.
fn withdrawn_cards(board: &cards::Cards, items: &[Approval], now: u64) -> Vec<Card> {
    items
        .iter()
        .filter_map(|item| {
            card_kind(&item.kind).map(|kind| board.withdraw(kind, &card_subject(&item.id), now))
        })
        .flatten()
        .collect()
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
    /// For a review decision: what the owner chose (see `review_graph::owner_ruled`).
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
    // The decision is written before anything it starts (a review, a call's answer).
    crate::store::saver::save_now(app, "approvals");
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
    withdraw_cards(app, std::slice::from_ref(&item));
    // The decision is recorded; each kind then does its own work with it.
    let applied = match item.kind.as_str() {
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
    // The mode the gate enforces, and where it comes from (see `Store::effective`).
    let project = crate::project_settings::of_agent(app, cid).and_then(|s| s.permission_mode);
    let (mode, source) = app.approvals.effective(cid, project);
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
    app.approvals.set_mode(cid, body.mode);
    // The agent's policy follows its mode: only an autonomous agent's VM runs calls on its own.
    crate::policy::push(app, cid);
    // The owner's change is written before the answer.
    let saved = crate::store::saver::save_now(app, "approvals");
    Response::json(200, &mode_change_body(&app.approvals, body.mode, saved))
}

/// The answer to a mode change: the mode, and whether it was written to the hub's store (`saved`, what the write
/// said). While the saved modes are unreadable the change applies in memory only, and the answer says so with a note.
fn mode_change_body(store: &Store, mode: Mode, saved: bool) -> serde_json::Value {
    if store.damaged() {
        json!({
            "mode": mode,
            "saved": false,
            "note": "the saved permission modes could not be read at startup; the change applies until the hub restarts",
        })
    } else {
        json!({ "mode": mode, "saved": saved })
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

    /// The store as the hub saves it and loads it back: its rows written to an in-memory `approval_queue`, then a fresh
    /// store loaded from them, as a restart does.
    fn restarted(store: &Store) -> Store {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let loaded = Store::default();
        loaded.load(&conn).unwrap();
        loaded
    }

    #[test]
    fn a_store_that_cannot_be_read_asks_for_every_agent_and_writes_nothing() {
        // A connection with no tables: the saved modes cannot be read.
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        let store = Store::default();
        assert!(store.load(&conn).is_err());
        assert!(store.damaged());
        assert_eq!(store.mode(7), Mode::Ask);
        assert_eq!(
            store.effective(7, None),
            (Mode::Ask, "damaged-file-fallback")
        );
        // The gate, too: an agent with no saved mode and an autonomous project is held, not run.
        assert!(matches!(
            store.gate_with(
                7,
                "write_thing",
                "write",
                &json!({}),
                &Held::default(),
                Mode::Autonomous
            ),
            Gate::Held { .. }
        ));
        // A mode change applies in memory; the answer says it was not saved, and nothing is written over the store.
        store.set_mode(7, Mode::ReadOnly);
        assert_eq!(store.mode(7), Mode::ReadOnly);
        assert_eq!(
            mode_change_body(&store, Mode::ReadOnly, true)["saved"],
            false
        );
        assert!(
            store.rows().unwrap().tables.is_empty(),
            "an unreadable store writes nothing"
        );
        // A store that reads fine gives the default to an agent with no mode.
        assert_eq!(Store::default().mode(7), Mode::Autonomous);
        assert_eq!(
            Store::default().effective(7, None),
            (Mode::Autonomous, "default")
        );
    }

    #[test]
    fn modes_round_trip_through_the_store() {
        let store = Store::default();
        assert_eq!(store.mode(7), Mode::Autonomous);
        store.set_mode(7, Mode::ReadOnly);
        store.set_mode(9, Mode::Ask);

        let again = restarted(&store);
        assert_eq!(again.mode(7), Mode::ReadOnly);
        assert_eq!(again.mode(9), Mode::Ask);
        assert_eq!(again.mode(3), Mode::Autonomous);
        assert_eq!(mode_change_body(&again, Mode::Ask, true)["saved"], true);
        assert_eq!(mode_change_body(&again, Mode::Ask, false)["saved"], false);
    }

    #[test]
    fn ask_holds_a_call_until_approved_then_passes_once() {
        let store = Store::default();
        store.set_mode(1, Mode::Ask);
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
    }

    #[test]
    fn read_only_refuses_writes() {
        let store = Store::default();
        store.set_mode(2, Mode::ReadOnly);
        let (refusal, created) = store.gate_plain(2, "write_thing", "write", &json!({}));
        assert_eq!(
            refusal.unwrap(),
            "this agent is read-only: write_thing is not allowed"
        );
        assert!(created.is_none());
    }

    #[test]
    fn reads_pass_in_every_mode_and_autonomous_lets_writes_run() {
        let store = Store::default();
        for (cid, mode) in [(1, Mode::Autonomous), (2, Mode::Ask), (3, Mode::ReadOnly)] {
            store.set_mode(cid, mode);
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
    }

    #[test]
    fn the_modes_and_the_queue_come_back_whole_from_their_tables() {
        let store = Store::default();
        store.set_mode(5, Mode::Ask);
        let (_, created) = store.gate_plain(5, "write_thing", "write", &json!({ "a": 1 }));
        let id = created.unwrap().id;

        let other = restarted(&store);
        assert_eq!(
            serde_json::to_value(other.list()).unwrap(),
            serde_json::to_value(store.list()).unwrap()
        );
        assert_eq!(other.mode(5), Mode::Ask);
        assert_eq!(other.list()[0].id, id);
        // The id counter comes back too: the next item gets the id after the last one.
        let (_, next) = other.gate_plain(5, "write_thing", "write", &json!({ "a": 2 }));
        assert_eq!(next.unwrap().id, "a2");
    }

    #[test]
    fn submit_queues_a_pending_decision_in_any_mode() {
        let store = Store::default();
        store.set_mode(6, Mode::ReadOnly);
        let detail = json!({ "cid": 6, "amount": 900, "to": "bob" });
        let item = store.submit(6, "tool", "run ls", detail.clone());
        assert_eq!(item.state, State::Pending);
        assert_eq!(item.kind, "tool");
        assert_eq!(item.subject, "run ls");
        assert_eq!(item.tool, "run ls");
        assert_eq!(item.detail, detail);
        assert_eq!(item.input, detail);
        assert_eq!(item.cid, 6);
        assert_eq!(store.list()[0].id, item.id);
        // Gate and submit share one id counter: the held call gets the next id.
        store.set_mode(7, Mode::Ask);
        let (_, held) = store.gate_plain(7, "write_thing", "write", &json!({}));
        let held = held.expect("a held call queues an item");
        let next: u64 = item.id[1..].parse::<u64>().unwrap() + 1;
        assert_eq!(held.id, format!("a{next}"));
    }

    #[test]
    fn submit_holds_the_queue_cap() {
        let store = Store::default();
        for i in 0..(QUEUE_CAP + 5) {
            store.submit(1, "tool", &format!("run {i}"), json!({ "cid": 1 }));
        }
        let list = store.list();
        assert_eq!(list.len(), QUEUE_CAP);
        // Newest first: the five oldest were dropped.
        assert_eq!(list[0].subject, format!("run {}", QUEUE_CAP + 4));
        assert_eq!(list[QUEUE_CAP - 1].subject, "run 5");
    }

    #[test]
    fn a_held_engine_call_keeps_its_thread_and_waiter_and_joins_a_pending_item() {
        let store = Store::default();
        store.set_mode(8, Mode::Ask);
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
    }

    #[test]
    fn a_decision_is_pushed_to_each_waiter_and_a_failed_push_is_reported() {
        let store = Store::default();
        store.set_mode(4, Mode::Ask);
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

    fn held_item(id: &str, kind: &str, detail: Value) -> Approval {
        Approval {
            id: id.into(),
            cid: 3,
            tool: "write_thing".into(),
            input: json!({ "path": "/x" }),
            created_ms: 0,
            state: State::Pending,
            reason: None,
            kind: kind.into(),
            subject: "write_thing".into(),
            detail,
            thread: None,
            waiting: Vec::new(),
        }
    }

    #[test]
    fn a_tool_call_card_is_answered_into_a_decision_and_withdrawn_when_decided_elsewhere() {
        let store = Store::default();
        let board = cards::Cards::default();
        store.set_mode(3, Mode::Ask);
        let (_, created) = store.gate_plain(3, "write_thing", "write", &json!({ "path": "/x" }));
        let item = created.expect("the held call queues its approval");
        let card = board.open(tool_content(&item, "builder #3"), 1).0;
        assert_eq!(card.kind, cards::Kind::ToolCall);
        assert_eq!(card.subject, card_subject(&item.id));
        assert_eq!(
            card.choices
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["approve", "deny"]
        );

        // Answering the card rules the approval, and the store decides it.
        board.answer(&card.id, "approve", None, 2).unwrap();
        let (approve, action) = ruling(card.kind, "approve").unwrap();
        assert_eq!((approve, action), (true, None));
        assert_eq!(
            store.decide(&item.id, approve, None).unwrap().state,
            State::Approved
        );

        // A second call decided through the approvals route: its card is withdrawn, not answered.
        let (_, created) = store.gate_plain(3, "write_other", "write", &json!({}));
        let other = created.expect("the second held call queues its approval");
        let open = board.open(tool_content(&other, "builder #3"), 3).0;
        let decided = store.decide(&other.id, false, None).unwrap();
        let withdrawn = withdrawn_cards(&board, std::slice::from_ref(&decided), 4);
        assert_eq!(
            withdrawn.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            [open.id.as_str()]
        );
        assert_eq!(board.get(&open.id).unwrap().state, cards::State::Withdrawn);
        assert!(board.list(true, None).is_empty());
    }

    #[test]
    fn a_review_card_offers_send_back_only_when_a_review_run_can_take_it() {
        let run = held_item(
            "a9",
            "review",
            json!({ "task": "t7", "run": "r1", "verdict": "two checks failed", "summary": "built it" }),
        );
        let card = review_content(&run);
        assert_eq!(card.task.as_deref(), Some("t7"));
        assert_eq!(
            card.choices
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["pass", "rework", "fail"]
        );
        assert_eq!(card.body.as_deref(), Some("two checks failed built it"));
        assert_eq!(ruling(card.kind, "pass"), Some((true, None)));
        assert_eq!(ruling(card.kind, "rework"), Some((false, Some("rework"))));
        assert_eq!(ruling(card.kind, "fail"), Some((false, None)));

        // A ruling from before the review graph has no run to send back.
        let legacy = held_item("a10", "review", json!({ "task": "t8", "summary": "done" }));
        let card = review_content(&legacy);
        assert_eq!(
            card.choices
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["pass", "fail"]
        );
        assert!(
            !card.choices.iter().any(|c| c.id == "rework"),
            "a card offers only the choices it has: none can send a legacy ruling back"
        );
    }

    #[test]
    fn only_tool_calls_and_review_rulings_have_cards() {
        assert_eq!(card_kind("tool"), Some(cards::Kind::ToolCall));
        assert_eq!(card_kind("review"), Some(cards::Kind::Review));
        assert_eq!(card_kind("payment"), None);
        let payment = held_item("a11", "payment", json!({}));
        assert!(withdrawn_cards(&cards::Cards::default(), &[payment], 1).is_empty());
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("ab", 3), "ab");
    }
}
