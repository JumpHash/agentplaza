//! The Approvals screen (parity with the web console's `approvals` view): the tool calls agents
//! wait on the owner's yes or no, the agents the orchestrator proposes to spawn (to review or
//! reject), and the decided history. Reloads on every poll tick; reasons typed but not sent survive
//! the reloads.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::{column, container, row, text, text_input, Space, Stack};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task, Theme};
use serde_json::{json, Value};

use super::wizard::{self, ProposalView, WizardMsg};
use super::{sanctions, Id};
use crate::console::{client, ui, Ctx};
use crate::theme::{SERIF};

const STRONG: Font = Font { weight: font::Weight::Semibold, ..SERIF };

/// A section heading (the web's `.approvals-subhead`: mono semibold caps).
fn subhead<'a>(title: &str) -> Element<'a, Msg> {
    text(title.to_uppercase()).font(ui::MONO_BOLD).size(16.0).color(crate::theme::palette().ink).into()
}

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
    /// The sanction ids of the suspensions in force: an escalation offers Lift only for these.
    suspensions: HashSet<u64>,
    /// Reasons typed but not yet sent, keyed `ap-<id>` (approval) or `pp-<id>` (proposal).
    drafts: HashMap<String, String>,
    /// Approvals whose decision is in flight (their buttons are disabled).
    busy: HashSet<String>,
    /// The proposal whose rejection reason is being typed.
    rejecting: Option<String>,
    /// The collapsed "Decided" list of proposals is open.
    proposals_open: bool,
    pub error: Option<String>,
    /// Answers still due from the running load (four calls); 0 when idle.
    pending: u8,
    /// A reload was asked for while a load ran: it starts once the running one settles.
    again: bool,
    /// The new-agent wizard, opened on a proposal's Review (it approves or rejects the proposal).
    wizard: wizard::Wizard,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Approvals(Result<Value, String>),
    Proposals(Result<Value, String>),
    Agents(Result<Value, String>),
    Sanctions(Result<sanctions::Register, String>),
    /// A hub event: the approval and agent_proposal topics reload the screen.
    Event(crate::console::events::Event),
    /// A reason typed for a draft key.
    Draft(String, String),
    /// Approve (true) or deny (false) an approval; `action` picks the sanction call explicitly
    /// (`acknowledge`, `lift` or `retire`) and is None for tool and payment approvals.
    Decide(String, bool, Option<&'static str>),
    Decided(String, Result<(), String>),
    RejectOpen(String),
    RejectCancel,
    RejectConfirm(String),
    Rejected(String, Result<(), String>),
    ToggleDecided,
    /// The proposal wizard's messages (Review opens it; Done(Ok) reloads the list).
    Wizard(WizardMsg),
}

#[derive(Debug, Clone)]
struct Approval {
    id: String,
    cid: u64,
    /// "tool", "sanction" or "payment"; a tool approval is "tool".
    kind: String,
    /// What the approval is about: the tool for a tool approval.
    subject: String,
    tool: String,
    input: Value,
    /// The kind's own detail (a sanction's sanction_id, a payment's amount); the input for a tool.
    detail: Value,
    created_ms: i64,
    state: String,
    reason: String,
}

impl Approval {
    fn from_value(v: &Value) -> Option<Self> {
        let tool = field(v, "tool");
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            cid: v.get("cid")?.as_u64()?,
            kind: Some(field(v, "kind")).filter(|k| !k.is_empty()).unwrap_or_else(|| "tool".to_string()),
            subject: Some(field(v, "subject")).filter(|s| !s.is_empty()).unwrap_or_else(|| tool.clone()),
            input: v.get("input").cloned().unwrap_or(Value::Null),
            detail: v.get("detail").cloned().unwrap_or(Value::Null),
            tool,
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
    /// The proposal as the wizard reviews it (its Review opens the wizard on it).
    review: Option<ProposalView>,
}

impl Proposal {
    fn from_value(v: &Value) -> Option<Self> {
        let role = v.get("role").cloned().unwrap_or(Value::Null);
        let title = Some(field(&role, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| field(&role, "id"));
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            review: serde_json::from_value(v.clone()).ok(),
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
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    // One load at a time: a reload asked for meanwhile runs once the four answers are in.
    if state.pending > 0 {
        state.again = true;
        return Task::none();
    }
    state.pending = 4;
    let (c1, c2, c3, c4) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    Task::batch([
        client::task(move || c1.get::<Value>("/v1/approvals"), Msg::Approvals),
        client::task(move || c2.get::<Value>("/v1/agent-proposals"), Msg::Proposals),
        client::task(move || c3.get::<Value>("/v1/agents"), Msg::Agents),
        client::task(move || sanctions::list(&c4, None, None, None), Msg::Sanctions),
    ])
}

/// One of the four load answers arrived; the last one starts a reload that was asked for meanwhile.
fn settled(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.pending = state.pending.saturating_sub(1);
    if state.pending == 0 && std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    }
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
            settled(state, ctx)
        }
        Msg::Proposals(res) => {
            state.proposals = res.map(|v| list(&v, Proposal::from_value)).unwrap_or_default();
            settled(state, ctx)
        }
        Msg::Agents(res) => {
            if let Ok(v) = res {
                state.agents = list(&v, agent_entry);
            }
            settled(state, ctx)
        }
        Msg::Sanctions(res) => {
            if let Ok(register) = res {
                state.suspensions = register.suspensions.iter().filter(|s| s.active).map(|s| s.id).collect();
            }
            settled(state, ctx)
        }
        Msg::Event(event) => match event.kind.as_str() {
            "approval" | "agent_proposal" => load(state, ctx),
            _ => Task::none(),
        },
        Msg::Draft(key, text) => {
            state.drafts.insert(key, text);
            Task::none()
        }
        Msg::Decide(id, approve, action) => {
            state.busy.insert(id.clone());
            let mut body = json!({ "approve": approve });
            if let Some(action) = action {
                body["action"] = json!(action);
            }
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
        Msg::Wizard(m) => {
            // A proposal approved or rejected: the wizard closes and the list reloads.
            let reload = matches!(m, WizardMsg::Done(Ok(())));
            let step = wizard::update(&mut state.wizard, m, ctx).map(Msg::Wizard);
            if reload {
                Task::batch([step, load(state, ctx)])
            } else {
                step
            }
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let page = page(state);
    if state.wizard.open {
        Stack::new().push(page).push(wizard::view(&state.wizard, ctx).map(Msg::Wizard)).into()
    } else {
        page
    }
}

fn page<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if !state.proposals.is_empty() {
        parts.push(proposals_view(state));
    }
    parts.push(pending_view(state));
    parts.push(subhead("Decided"));
    parts.push(history_view(state));
    ui::sheet(ui::view_head("Approvals", vec![]), column(parts).spacing(22).width(Length::Fill).into())
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
    // The owner's calls: a sanction offers acknowledge, lift (only while its suspension holds) and
    // retire; a payment and a tool keep approve and deny.
    let decide = |approve: bool, action: Option<&'static str>| (!busy).then(|| Msg::Decide(a.id.clone(), approve, action));
    let calls: Vec<Element<'a, Msg>> = match a.kind.as_str() {
        "sanction" => {
            let lift = a.detail.get("sanction_id").and_then(Value::as_u64).filter(|id| state.suspensions.contains(id));
            let mut calls = vec![ui::btn("Retire", None, ui::Kind::Danger, decide(false, Some("retire")))];
            if lift.is_some() {
                calls.push(ui::btn("Lift", None, ui::Kind::Secondary, decide(false, Some("lift"))));
            }
            calls.push(ui::btn("Acknowledge", None, ui::Kind::Primary, decide(true, Some("acknowledge"))));
            calls
        }
        _ => vec![ui::btn("Deny", None, ui::Kind::Danger, decide(false, None)), ui::btn("Approve", None, ui::Kind::Primary, decide(true, None))],
    };
    let title: Element<'a, Msg> = match a.kind.as_str() {
        "sanction" => ui::prose(format!("Escalation: {}", a.subject)).into(),
        "payment" => ui::prose(format!("Payment awaiting approval: {}", a.subject)).into(),
        _ => ui::mono(a.tool.clone(), 12.0, crate::theme::palette().ink).into(),
    };
    let head = row![
        ui::btn(&name_of(&state.agents, a.cid), None, ui::Kind::Ghost, Some(Msg::Go(Id::Agent(a.cid)))),
        title,
        Space::new().width(Length::Fill),
        ui::faint(ago(a.created_ms)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let shown = if a.kind == "tool" { &a.input } else { &a.detail };
    let input = serde_json::to_string_pretty(shown).unwrap_or_else(|_| "null".to_string());
    let mut actions = row![Space::new().width(Length::Fill)].spacing(8).align_y(Alignment::Center);
    for call in calls {
        actions = actions.push(call);
    }
    // A sanction shows no input: its subject is the escalation, and the note says what Retire records.
    let mut stack = column![head];
    if a.kind != "sanction" {
        stack = stack.push(code_box(input));
    }
    stack = stack.push(column![reason, actions].spacing(8));
    if a.kind == "sanction" {
        stack = stack.push(ui::faint("Retire only records your decision. apid does not retire the agent."));
    }
    frame(stack.spacing(10).into())
}

/// One decided approval, as a line of the history.
fn history_row<'a>(state: &'a State, a: &'a Approval) -> Element<'a, Msg> {
    let (label, tone) = match a.state.as_str() {
        "approved" => ("Approved", ui::Tone::Ok),
        "denied" => ("Denied", ui::Tone::Danger),
        "used" => ("Used", ui::Tone::Info),
        _ => ("Waiting", ui::Tone::Warn),
    };
    let line = row![
        ui::badge(label, tone),
        ui::mono(if a.subject.is_empty() { a.tool.clone() } else { a.subject.clone() }, 12.0, crate::theme::palette().ink),
        ui::prose(name_of(&state.agents, a.cid)),
        reason_or_gap(&a.reason),
        ui::faint(ago(a.created_ms)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    line.into()
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

    let mut items: Vec<Element<'a, Msg>> = vec![subhead("New agents proposed by the orchestrator")];
    if waiting.is_empty() {
        items.push(ui::faint("No proposals waiting.").into());
    } else {
        items.extend(waiting.into_iter().map(|p| proposal_card(state, p)));
    }
    if !past.is_empty() {
        items.push(ui::disclosure(format!("Decided ({})", past.len()), crate::theme::palette().ink_2, state.proposals_open, Msg::ToggleDecided));
        if state.proposals_open {
            items.extend(past.into_iter().map(proposal_row));
        }
    }
    column(items).spacing(12).into()
}

/// One proposed agent, waiting: what it is for, why the orchestrator wants it, and the owner's call.
fn proposal_card<'a>(state: &'a State, p: &'a Proposal) -> Element<'a, Msg> {
    let mut head = row![text(p.title.clone()).font(STRONG).size(15.0).color(crate::theme::palette().ink)].spacing(10).align_y(Alignment::Center);
    if p.new_role {
        head = head.push(ui::badge("new role", ui::Tone::Signal));
    }
    head = head.push(ui::badge(&format!("{} {}", p.count, if p.count == 1 { "agent" } else { "agents" }), ui::Tone::Info));
    head = head.push(Space::new().width(Length::Fill));
    head = head.push(ui::faint(ago(p.created_ms)));

    let mut body = column![head, ui::prose(p.reason.clone())].spacing(8);
    if !p.instruction.is_empty() {
        body = body.push(ui::prose(format!("“{}”", p.instruction)));
    }
    body = body.push(ui::faint(format!("Proposed by {}", p.by)));
    // The review opens the new-agent wizard on this proposal (approve or reject there).
    let review = ui::btn("Review…", None, ui::Kind::Primary, p.review.clone().map(|v| Msg::Wizard(WizardMsg::OpenProposal(v))));
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
    let line = row![
        ui::badge(label, tone),
        text(p.title.clone()).font(STRONG).size(14.0).color(crate::theme::palette().ink),
        reason_or_gap(&p.note),
        ui::faint(ago(if p.decided_ms != 0 { p.decided_ms } else { p.created_ms })),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    line.into()
}

/// The reason, filling the line so the time sits at its right end (the web's `.approval-row-reason`).
fn reason_or_gap<'a>(reason: &str) -> Element<'a, Msg> {
    if reason.is_empty() {
        Space::new().width(Length::Fill).into()
    } else {
        container(ui::faint(reason.to_string())).width(Length::Fill).into()
    }
}

/// A card's frame: a 1px rule around its body.
fn frame<'a>(body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(body)
        .padding(12)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// The tool input, as pretty JSON on paper.
fn code_box<'a>(source: String) -> Element<'a, Msg> {
    container(ui::mono(source, 13.0, crate::theme::palette().ink_2))
        .padding(Padding { top: 10.0, right: 12.0, bottom: 10.0, left: 12.0 })
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(Color::from_rgb8(0x1e, 0x23, 0x32).into()),
            border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// The hub events this screen follows live (the web refreshes on the same topics).
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}
