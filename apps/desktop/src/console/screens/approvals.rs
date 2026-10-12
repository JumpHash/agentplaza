//! The Approvals screen (parity with the web console's `approvals` view): the tool calls agents
//! wait on the owner's yes or no, the agents the orchestrator proposes to spawn (to review or
//! reject), and the decided history. Reloads on every poll tick; reasons typed but not sent survive
//! the reloads.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{column, container, row, text_input, Space};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};

#[derive(Debug, Default)]
pub struct State {
    /// The first load has answered (until then the pending list is blank, as on the web).
    loaded: bool,
    /// The approvals list could not be read.
    failed: bool,
    approvals: Vec<Approval>,
    proposals: Vec<Proposal>,
    /// (cid, name) of every agent, to name the agents the approvals belong to.
    agents: Vec<(u64, String)>,
    /// Reasons typed but not yet sent, keyed `ap-<id>` (approval) or `pp-<id>` (proposal).
    drafts: HashMap<String, String>,
    /// Approvals whose decision is in flight (their buttons are disabled).
    busy: HashSet<String>,
    /// The proposal whose rejection reason is being typed.
    rejecting: Option<String>,
    /// The collapsed "Decided" list of proposals is open.
    proposals_open: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Approvals(Result<Value, String>),
    Proposals(Result<Value, String>),
    Agents(Result<Value, String>),
    /// A reason typed for a draft key.
    Draft(String, String),
    /// Approve (true) or deny (false) an approval.
    Decide(String, bool),
    Decided(String, Result<(), String>),
    RejectOpen(String),
    RejectCancel,
    RejectConfirm(String),
    Rejected(String, Result<(), String>),
    ToggleDecided,
}

#[derive(Debug, Clone)]
struct Approval {
    id: String,
    cid: u64,
    tool: String,
    input: Value,
    created_ms: i64,
    state: String,
    reason: String,
}

impl Approval {
    fn from_value(v: &Value) -> Option<Self> {
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            cid: v.get("cid")?.as_u64()?,
            tool: field(v, "tool"),
            input: v.get("input").cloned().unwrap_or(Value::Null),
            created_ms: v.get("created_ms").and_then(Value::as_i64).unwrap_or(0),
            state: field(v, "state"),
            reason: field(v, "reason"),
        })
    }
}

#[derive(Debug, Clone)]
struct Proposal {
    id: String,
    /// The role's title, or its id when it has none.
    title: String,
    new_role: bool,
    count: u64,
    reason: String,
    instruction: String,
    by: String,
    created_ms: i64,
    decided_ms: i64,
    state: String,
    note: String,
}

impl Proposal {
    fn from_value(v: &Value) -> Option<Self> {
        let role = v.get("role").cloned().unwrap_or(Value::Null);
        let title = Some(field(&role, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| field(&role, "id"));
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            title,
            new_role: v.get("new_role").and_then(Value::as_bool).unwrap_or(false),
            count: v.get("count").and_then(Value::as_u64).unwrap_or(1),
            reason: field(v, "reason"),
            instruction: field(v, "instruction"),
            by: field(v, "by"),
            created_ms: v.get("created_ms").and_then(Value::as_i64).unwrap_or(0),
            decided_ms: v.get("decided_ms").and_then(Value::as_i64).unwrap_or(0),
            state: field(v, "state"),
            note: field(v, "note"),
        })
    }
}

fn field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn list<T>(v: &Value, parse: fn(&Value) -> Option<T>) -> Vec<T> {
    v.as_array().map(|items| items.iter().filter_map(parse).collect()).unwrap_or_default()
}

fn agent_entry(v: &Value) -> Option<(u64, String)> {
    Some((v.get("cid")?.as_u64()?, field(v, "name")))
}

/// The agent's name as the web shows it: "Unnamed" when blank, "agent <cid>" when unknown.
fn name_of(agents: &[(u64, String)], cid: u64) -> String {
    match agents.iter().find(|(c, _)| *c == cid) {
        Some((_, name)) if name.trim().is_empty() => "Unnamed".to_string(),
        Some((_, name)) => name.clone(),
        None => format!("agent {cid}"),
    }
}

/// Relative time, as the web's `ago`.
fn ago(ms: i64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let s = ((now - ms) as f64 / 1000.0).round().max(0.0) as i64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let (c1, c2, c3) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    Task::batch([
        client::task(move || c1.get::<Value>("/v1/approvals"), Msg::Approvals),
        client::task(move || c2.get::<Value>("/v1/agent-proposals"), Msg::Proposals),
        client::task(move || c3.get::<Value>("/v1/agents"), Msg::Agents),
    ])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Approvals(res) => {
            match res {
                Ok(v) => {
                    state.approvals = list(&v, Approval::from_value);
                    state.failed = false;
                }
                Err(_) => state.failed = true,
            }
            state.loaded = true;
            Task::none()
        }
        Msg::Proposals(res) => {
            state.proposals = res.map(|v| list(&v, Proposal::from_value)).unwrap_or_default();
            Task::none()
        }
        Msg::Agents(res) => {
            if let Ok(v) = res {
                state.agents = list(&v, agent_entry);
            }
            Task::none()
        }
        Msg::Draft(key, text) => {
            state.drafts.insert(key, text);
            Task::none()
        }
        Msg::Decide(id, approve) => {
            state.busy.insert(id.clone());
            let mut body = json!({ "approve": approve });
            let reason = state.drafts.get(&format!("ap-{id}")).map(|r| r.trim().to_string()).unwrap_or_default();
            if !reason.is_empty() {
                body["reason"] = json!(reason);
            }
            let client = ctx.client.clone();
            let path = format!("/v1/approvals/{id}");
            client::task(move || client.post::<Value>(&path, &body), move |r| Msg::Decided(id.clone(), r.map(|_| ())))
        }
        Msg::Decided(id, res) => {
            state.busy.remove(&id);
            match res {
                Ok(()) => {
                    state.drafts.remove(&format!("ap-{id}"));
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(state, ctx)
        }
        Msg::RejectOpen(id) => {
            state.rejecting = Some(id);
            Task::none()
        }
        Msg::RejectCancel => {
            state.rejecting = None;
            Task::none()
        }
        Msg::RejectConfirm(id) => {
            let reason = state.drafts.get(&format!("pp-{id}")).map(|r| r.trim().to_string()).unwrap_or_default();
            state.rejecting = None;
            let client = ctx.client.clone();
            let path = format!("/v1/agent-proposals/{id}/reject");
            let body = json!({ "reason": reason });
            client::task(move || client.post::<Value>(&path, &body), move |r| Msg::Rejected(id.clone(), r.map(|_| ())))
        }
        Msg::Rejected(id, res) => {
            match res {
                Ok(()) => {
                    state.drafts.remove(&format!("pp-{id}"));
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(state, ctx)
        }
        Msg::ToggleDecided => {
            state.proposals_open = !state.proposals_open;
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if !state.proposals.is_empty() {
        parts.push(proposals_view(state));
    }
    parts.push(pending_view(state));
    parts.push(ui::mono("Decided", 13.0, ui::INK).into());
    parts.push(history_view(state));
    ui::sheet(ui::view_head("Approvals", vec![]), column(parts).spacing(22).into())
}

/// The waiting approvals, longest wait first, each a card with the owner's call.
fn pending_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if !state.loaded {
        return Space::new().into();
    }
    if state.failed {
        return ui::empty("alert", "Could not load approvals.", "");
    }
    let mut waiting: Vec<&Approval> = state.approvals.iter().filter(|a| a.state == "pending").collect();
    waiting.sort_by_key(|a| a.created_ms);
    if waiting.is_empty() {
        return ui::empty(
            "approval",
            "Nothing is waiting",
            "Agents ask here before acts that need your approval, and the orchestrator proposes new agents here.",
        );
    }
    column(waiting.into_iter().map(|a| entry(state, a))).spacing(12).into()
}

/// One waiting approval: who, what, the input, and the owner's reason and call.
fn entry<'a>(state: &'a State, a: &'a Approval) -> Element<'a, Msg> {
    let busy = state.busy.contains(&a.id);
    let key = format!("ap-{}", a.id);
    let draft = state.drafts.get(&key).map(String::as_str).unwrap_or("").to_string();
    let reason = text_input("Reason (optional)", &draft)
        .on_input(move |s| Msg::Draft(key.clone(), s))
        .size(13.0)
        .width(Length::Fill);
    let no = ui::btn("Deny", None, ui::Kind::Danger, (!busy).then(|| Msg::Decide(a.id.clone(), false)));
    let yes = ui::btn("Approve", None, ui::Kind::Primary, (!busy).then(|| Msg::Decide(a.id.clone(), true)));
    let head = row![
        ui::btn(&name_of(&state.agents, a.cid), None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(a.cid)))),
        ui::mono(a.tool.clone(), 12.0, ui::INK),
        ui::faint(ago(a.created_ms)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let input = serde_json::to_string_pretty(&a.input).unwrap_or_else(|_| "null".to_string());
    frame(
        column![
            head,
            code_box(input),
            column![reason, row![Space::new().width(Length::Fill), no, yes].spacing(8).align_y(Alignment::Center)].spacing(8),
        ]
        .spacing(10)
        .into(),
    )
}

/// One decided approval, as a line of the history.
fn history_row<'a>(state: &'a State, a: &'a Approval) -> Element<'a, Msg> {
    let (label, tone) = match a.state.as_str() {
        "approved" => ("Approved", ui::Tone::Ok),
        "denied" => ("Denied", ui::Tone::Danger),
        "used" => ("Used", ui::Tone::Info),
        _ => ("Waiting", ui::Tone::Warn),
    };
    let mut line = row![
        ui::badge(label, tone),
        ui::mono(a.tool.clone(), 12.0, ui::INK),
        ui::faint(name_of(&state.agents, a.cid)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if !a.reason.is_empty() {
        line = line.push(ui::faint(a.reason.clone()));
    }
    line.push(ui::faint(ago(a.created_ms))).into()
}

fn history_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut past: Vec<&Approval> = state.approvals.iter().filter(|a| a.state != "pending").collect();
    past.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
    if past.is_empty() {
        return ui::faint("No decisions yet.").into();
    }
    column(past.into_iter().map(|a| history_row(state, a))).spacing(6).into()
}

/// The agents the orchestrator proposes to spawn: waiting ones as cards, decided ones collapsed.
fn proposals_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut waiting: Vec<&Proposal> = state.proposals.iter().filter(|p| p.state == "pending").collect();
    waiting.sort_by_key(|p| p.created_ms);
    let mut past: Vec<&Proposal> = state.proposals.iter().filter(|p| p.state != "pending").collect();
    past.sort_by_key(|p| std::cmp::Reverse(p.created_ms));

    let mut items: Vec<Element<'a, Msg>> = vec![ui::mono("New agents proposed by the orchestrator", 13.0, ui::INK).into()];
    if waiting.is_empty() {
        items.push(ui::faint("No proposals waiting.").into());
    } else {
        items.extend(waiting.into_iter().map(|p| proposal_card(state, p)));
    }
    if !past.is_empty() {
        items.push(ui::btn(&format!("Decided ({})", past.len()), None, ui::Kind::Ghost, Some(Msg::ToggleDecided)));
        if state.proposals_open {
            items.extend(past.into_iter().map(proposal_row));
        }
    }
    column(items).spacing(12).into()
}

/// One proposed agent, waiting: what it is for, why the orchestrator wants it, and the owner's call.
fn proposal_card<'a>(state: &'a State, p: &'a Proposal) -> Element<'a, Msg> {
    let mut head = row![ui::mono(p.title.clone(), 13.0, ui::INK)].spacing(10).align_y(Alignment::Center);
    if p.new_role {
        head = head.push(ui::badge("new role", ui::Tone::Signal));
    }
    head = head.push(ui::badge(&format!("{} {}", p.count, if p.count == 1 { "agent" } else { "agents" }), ui::Tone::Info));
    head = head.push(ui::faint(ago(p.created_ms)));

    let mut body = column![head, ui::prose(p.reason.clone())].spacing(8);
    if !p.instruction.is_empty() {
        body = body.push(ui::prose(format!("“{}”", p.instruction)));
    }
    body = body.push(ui::faint(format!("Proposed by {}", p.by)));
    // The review opens the new-agent wizard, which is not ported yet (contract WIZ): disabled for now.
    let review = ui::btn("Review…", None, ui::Kind::Primary, None);
    let reject = ui::btn("Reject", None, ui::Kind::Danger, Some(Msg::RejectOpen(p.id.clone())));
    body = body.push(row![Space::new().width(Length::Fill), reject, review].spacing(8).align_y(Alignment::Center));

    if state.rejecting.as_deref() == Some(p.id.as_str()) {
        let key = format!("pp-{}", p.id);
        let draft = state.drafts.get(&key).map(String::as_str).unwrap_or("").to_string();
        let reason = text_input("Why reject?", &draft)
            .on_input(move |s| Msg::Draft(key.clone(), s))
            .on_submit(Msg::RejectConfirm(p.id.clone()))
            .size(13.0)
            .width(Length::Fill);
        body = body.push(
            column![
                reason,
                row![
                    Space::new().width(Length::Fill),
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::RejectCancel)),
                    ui::btn("Confirm reject", None, ui::Kind::Danger, Some(Msg::RejectConfirm(p.id.clone()))),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(8),
        );
    }
    frame(body.into())
}

/// One decided proposal, as a line of the collapsed history.
fn proposal_row<'a>(p: &'a Proposal) -> Element<'a, Msg> {
    let (label, tone) = match p.state.as_str() {
        "approved" => ("Approved", ui::Tone::Ok),
        "rejected" => ("Rejected", ui::Tone::Danger),
        _ => ("Waiting", ui::Tone::Warn),
    };
    let mut line = row![ui::badge(label, tone), ui::mono(p.title.clone(), 12.0, ui::INK)]
        .spacing(12)
        .align_y(Alignment::Center);
    if !p.note.is_empty() {
        line = line.push(ui::faint(p.note.clone()));
    }
    let when = if p.decided_ms != 0 { p.decided_ms } else { p.created_ms };
    line.push(ui::faint(ago(when))).into()
}

/// A card's frame: a 1px rule around its body.
fn frame<'a>(body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(body)
        .padding(12)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// The tool input, as pretty JSON on paper.
fn code_box<'a>(source: String) -> Element<'a, Msg> {
    container(ui::mono(source, 12.0, ui::INK_2))
        .padding(10)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(ui::PAPER.into()),
            border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}
