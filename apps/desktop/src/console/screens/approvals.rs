//! The Approvals screen, drawn as the web's approvals inbox (`apps/web/src/views/approvals.ts`): the waiting list on the
//! left (the orchestrator's proposed agents first, then the calls agents ask to make), the chosen one's card on the right,
//! and the decided ones folded under the list. A proposal's rejection asks for its reason in a dialog. Reloads on every
//! poll tick and hub event; reasons typed but not sent survive the reloads.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::text_editor::Content;
use iced::widget::{Space, Stack, button, column, container, row, text_editor};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task, Theme};
use serde_json::{Value, json};

use super::Id;
use super::wizard::{self, ProposalView, WizardMsg};
use crate::console::{Ctx, client, new_project::editor, rich, roles, scope, ui};
use crate::theme::{self, SANS_MEDIUM, SANS_SEMIBOLD};

#[derive(Debug, Default)]
pub struct State {
    /// The first load has answered (until then the screen is blank, as on the web).
    loaded: bool,
    /// The approvals list could not be read.
    failed: bool,
    approvals: Vec<Approval>,
    proposals: Vec<Proposal>,
    /// The agents as the hub sent them: their labels name the approvals' agents, and under a project, the approvals
    /// its agents asked for.
    roster: Vec<Value>,
    /// Reasons typed but not yet sent, keyed `ap-<id>` (approval) or `pp-<id>` (rejection of a proposal).
    reasons: HashMap<String, Content>,
    /// The editor a card shows before anything is typed in it.
    blank: Content,
    /// Approvals whose decision is in flight (their buttons are disabled).
    busy: HashSet<String>,
    /// The proposal whose rejection dialog is open.
    rejecting: Option<String>,
    /// The item on the right: the one picked, or the first waiting one until the owner picks another.
    selected: Option<String>,
    /// The collapsed "Decided proposals" and "Decided" lists are open.
    proposals_open: bool,
    history_open: bool,
    pub error: Option<String>,
    /// Answers still due from the running load (three calls); 0 when idle.
    pending: u8,
    /// A reload was asked for while a load ran: it starts once the running one settles.
    again: bool,
    /// The new-agent wizard, opened on a proposal's Review (it approves or rejects the proposal).
    wizard: wizard::Wizard,
    /// The rich content of the reasons and instructions the cards show: the hub's cards and images.
    cards: rich::Cards,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Approvals(Result<Value, String>),
    Proposals(Result<Value, String>),
    Agents(Result<Value, String>),
    /// A hub event: the approval and agent_proposal topics reload the screen.
    Event(crate::console::events::Event),
    /// An edit to a card's reason (or a rejection's reason), by its draft key.
    Edit(String, text_editor::Action),
    /// Opens the item on the right.
    Select(String),
    /// Approve (true) or deny (false) an approval; `action` picks the call explicitly (`rework`, a review's Send
    /// back) and is None for the plain Approve and Deny, and a review's Pass and Fail.
    Decide(String, bool, Option<&'static str>),
    Decided(String, Result<(), String>),
    RejectOpen(String),
    RejectCancel,
    RejectConfirm(String),
    Rejected(String, Result<(), String>),
    ToggleProposalHistory,
    ToggleHistory,
    /// The proposal wizard's messages (Review opens it; Done(Ok) reloads the list).
    Wizard(WizardMsg),
    /// A rich-content click (a chip, a link, an Open button) or a load of its cards and images.
    Rich(rich::Ev),
}

#[derive(Debug, Clone)]
pub(super) struct Approval {
    pub(super) id: String,
    pub(super) cid: u64,
    /// "tool", "review", "artifact.share" and the rest; a tool approval is "tool".
    pub(super) kind: String,
    /// What the approval is about: the tool for a tool approval.
    pub(super) subject: String,
    pub(super) tool: String,
    pub(super) input: Value,
    /// The kind's own detail (a review's run and task, a share's options); the input for a tool.
    pub(super) detail: Value,
    pub(super) created_ms: i64,
    pub(super) state: String,
    pub(super) reason: String,
}

impl Approval {
    pub(super) fn from_value(v: &Value) -> Option<Self> {
        let tool = field(v, "tool");
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            cid: v.get("cid")?.as_u64()?,
            kind: Some(field(v, "kind"))
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "tool".to_string()),
            subject: Some(field(v, "subject"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| tool.clone()),
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
pub(super) struct Proposal {
    pub(super) id: String,
    /// The role's title, or its id when it has none.
    pub(super) title: String,
    pub(super) new_role: bool,
    pub(super) count: u64,
    pub(super) reason: String,
    pub(super) instruction: String,
    pub(super) by_cid: u64,
    pub(super) created_ms: i64,
    pub(super) decided_ms: i64,
    pub(super) state: String,
    pub(super) note: String,
    /// The proposal as the wizard reviews it (its Review opens the wizard on it).
    pub(super) review: Option<ProposalView>,
}

impl Proposal {
    pub(super) fn from_value(v: &Value) -> Option<Self> {
        let role = v.get("role").cloned().unwrap_or(Value::Null);
        let title = Some(field(&role, "title"))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| field(&role, "id"));
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            review: serde_json::from_value(v.clone()).ok(),
            title,
            new_role: v.get("new_role").and_then(Value::as_bool).unwrap_or(false),
            count: v.get("count").and_then(Value::as_u64).unwrap_or(1),
            reason: field(v, "reason"),
            instruction: field(v, "instruction"),
            by_cid: v.get("by_cid").and_then(Value::as_u64).unwrap_or(0),
            created_ms: v.get("created_ms").and_then(Value::as_i64).unwrap_or(0),
            decided_ms: v.get("decided_ms").and_then(Value::as_i64).unwrap_or(0),
            state: field(v, "state"),
            note: field(v, "note"),
        })
    }
}

fn field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn list<T>(v: &Value, parse: fn(&Value) -> Option<T>) -> Vec<T> {
    v.as_array()
        .map(|items| items.iter().filter_map(parse).collect())
        .unwrap_or_default()
}

/// The agent's label as the web shows it: its role and its cid; `agent #<cid>` when the list does not have it.
fn label_of(roster: &[Value], cid: u64) -> String {
    match roster.iter().find(|a| a["cid"].as_u64() == Some(cid)) {
        Some(agent) => roles::agent_label_of(agent),
        None => roles::agent_label("", cid),
    }
}

/// Relative time, as the web's `ago`.
pub(super) fn ago(ms: i64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let s = ((now - ms) as f64 / 1000.0).round().max(0.0) as i64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The card's title for an approval, as the web's `waitTitle`.
fn wait_title(a: &Approval) -> String {
    let goal = field(&a.detail, "goal");
    match a.kind.as_str() {
        "review" if !goal.is_empty() => goal,
        "review" => format!("Review of {}", field(&a.detail, "task")),
        "deploy" => a.subject.clone(),
        "artifact.share" => format!("Share {}", a.subject),
        _ => match (
            a.input.get("command").and_then(Value::as_str),
            a.detail.as_str(),
        ) {
            (Some(command), _) => format!("{}: {command}", a.tool),
            (None, Some(detail)) => format!("{}: {detail}", a.tool),
            (None, None) => a.tool.clone(),
        },
    }
}

/// The icon of an approval's kind, as the web's `KIND_ICON`; the shield for the rest.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "review" => "git-pull-request",
        "tool" => "terminal",
        "deploy" => "rocket",
        "proposal" => "user-plus",
        _ => "shield-check",
    }
}

/// A kind's colours, as the web's `.k-<kind>`: its icon's ink on its soft ground.
fn kind_colors(kind: &str) -> (Color, Color) {
    let p = theme::palette();
    match kind {
        "review" => (p.review, p.review_soft),
        "tool" => (p.warn, p.warn_soft),
        "deploy" => (p.crit, p.crit_soft),
        "proposal" => (p.signal_ink, p.signal_soft),
        _ => (p.ink_2, p.hover),
    }
}

/// A rule of the console's line colour, across.
fn rule<'a>() -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

fn pad(vertical: f32, horizontal: f32) -> Padding {
    Padding {
        top: vertical,
        right: horizontal,
        bottom: vertical,
        left: horizontal,
    }
}

/// A kind's tile: its icon on the kind's soft ground, `size` square.
fn tile<'a>(
    icon_name: &str,
    kind: &str,
    size: f32,
    icon_size: f32,
    radius: f32,
) -> Element<'a, Msg> {
    let (fg, ground) = kind_colors(kind);
    container(ui::icon(icon_name, icon_size, fg))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center(Length::Fixed(size))
        .style(move |_: &Theme| container::Style {
            background: Some(ground.into()),
            border: Border::default().rounded(radius),
            ..Default::default()
        })
        .into()
}

/// A chip with a rule around it, as the web's `.ap-chip` and `.ap-tool`.
fn bordered<'a>(content: Element<'a, Msg>, padding: Padding) -> Element<'a, Msg> {
    container(content)
        .padding(padding)
        .style(|_: &Theme| container::Style {
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: theme::skin().radius.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A sheet-2 box, for JSON and quoted text (the web's `.ap-pre`, `.ap-quote`).
fn ground<'a>(content: Element<'a, Msg>, padding: Padding, radius: f32) -> Element<'a, Msg> {
    container(content)
        .padding(padding)
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(theme::palette().sheet_2.into()),
            border: Border::default().rounded(radius),
            ..Default::default()
        })
        .into()
}

/// A list row or fold button: the hover ground, the press ground when it is the one on the right.
fn row_button<'a>(
    content: Element<'a, Msg>,
    current: bool,
    msg: Msg,
    padding: Padding,
    radius: f32,
) -> Element<'a, Msg> {
    button(content)
        .on_press(msg)
        .width(Length::Fill)
        .padding(padding)
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let p = theme::palette();
            button::Style {
                background: if current {
                    Some(p.press.into())
                } else if hovered {
                    Some(p.hover.into())
                } else {
                    None
                },
                text_color: p.ink,
                border: Border::default().rounded(radius),
                ..Default::default()
            }
        })
        .into()
}

/// The web's `.ibx-sec`: a section label in the list, `top` above it.
fn section<'a>(text: &str, top: f32) -> Element<'a, Msg> {
    container(ui::label(text))
        .padding(Padding {
            top,
            right: 10.0,
            bottom: 4.0,
            left: 10.0,
        })
        .width(Length::Fill)
        .into()
}

/// A note in the list or the card's place when there is nothing: faint, in its own padding.
fn note<'a>(text: &str) -> Element<'a, Msg> {
    container(ui::faint(text.to_string()))
        .padding(pad(16.0, 16.0))
        .width(Length::Fill)
        .into()
}

/// A list row: the kind's tile, the title, and who asked and when; it opens its card on the right.
fn list_item<'a>(
    icon_name: &str,
    kind: &str,
    title: String,
    who: String,
    created: i64,
    current: bool,
    msg: Msg,
) -> Element<'a, Msg> {
    let body = column![
        ui::sans(ui::ellipsize(&title, 44), 13.5, theme::palette().ink).font(SANS_MEDIUM),
        ui::faint(format!("{who} · {}", ago(created))),
    ]
    .spacing(2)
    .width(Length::Fill);
    row_button(
        row![tile(icon_name, kind, 30.0, 16.0, 8.0), body]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        current,
        msg,
        pad(10.0, 10.0),
        10.0,
    )
}

/// A fold of the decided lists: a chevron and the count, in faint text.
fn fold<'a>(label: String, msg: Msg) -> Element<'a, Msg> {
    row_button(
        row![
            ui::icon("chevron-right", 13.0, theme::palette().ink_3),
            ui::sans(label, crate::theme::TYPE_META, theme::palette().ink_3),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        false,
        msg,
        pad(10.0, 10.0),
        8.0,
    )
}

/// Decided rows, each with a rule above it, as the web's `.ibx-dec .ap-row`.
fn ruled<'a>(rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    column(rows.into_iter().map(|r| column![rule(), r].into()))
        .spacing(0)
        .width(Length::Fill)
        .into()
}

/// The card's head: the kind's tile, the title and its chips, and the age at the right (the web's `.ibd-head`).
fn card_head<'a>(
    tile: Element<'a, Msg>,
    title: String,
    chips: Vec<Element<'a, Msg>>,
    created: i64,
) -> Element<'a, Msg> {
    let mut titles = column![
        ui::sans(title, 20.0, theme::palette().ink)
            .font(SANS_SEMIBOLD)
            .width(Length::Fill)
    ]
    .spacing(6)
    .width(Length::Fill);
    if !chips.is_empty() {
        titles = titles.push(ui::chip_row(chips));
    }
    row![
        tile,
        titles,
        container(ui::faint(ago(created))).padding(Padding {
            top: 6.0,
            ..Default::default()
        }),
    ]
    .spacing(12)
    .align_y(Alignment::Start)
    .into()
}

/// The card's foot: a rule, then the reason and the calls at its right (the web's `.ibd-foot`).
fn card_foot<'a>(reason: Element<'a, Msg>, calls: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut actions = row![Space::new().width(Length::Fill)]
        .spacing(8)
        .align_y(Alignment::Center);
    for call in calls {
        actions = actions.push(call);
    }
    column![
        rule(),
        column![reason, actions].spacing(8).padding(pad(12.0, 0.0)),
    ]
    .width(Length::Fill)
    .into()
}

/// The card: its head, its body (at most 640 wide, as the web's 68 characters), its foot.
fn card<'a>(
    head: Element<'a, Msg>,
    body: Element<'a, Msg>,
    foot: Element<'a, Msg>,
) -> Element<'a, Msg> {
    column![
        head,
        container(body).max_width(640.0).width(Length::Fill),
        foot,
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

/// The reason box of a card or a rejection: two lines, filled from the draft kept under its key.
fn reason_box<'a>(
    state: &'a State,
    key: &str,
    busy: bool,
    placeholder: &str,
    height: f32,
) -> Element<'a, Msg> {
    let content = state.reasons.get(key).unwrap_or(&state.blank);
    let editor_base = text_editor(content)
        .placeholder(placeholder.to_string())
        .height(Length::Fixed(height))
        .style(editor);
    if busy {
        editor_base.into()
    } else {
        let key = key.to_string();
        editor_base
            .on_action(move |action| Msg::Edit(key.clone(), action))
            .into()
    }
}

/// The reason typed under a key, trimmed; empty when none is.
fn reason_of(state: &State, key: &str) -> String {
    state
        .reasons
        .get(key)
        .map(|c| c.text().trim().to_string())
        .unwrap_or_default()
}

/// Asks the hub for the cards of the reasons and instructions on the screen that it does not hold yet.
fn prefetch(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let mut texts: Vec<String> = Vec::new();
    for p in &state.proposals {
        texts.push(p.reason.clone());
        texts.push(p.instruction.clone());
    }
    let refs: Vec<&str> = texts
        .iter()
        .filter(|t| !t.is_empty())
        .map(String::as_str)
        .collect();
    let rctx = rich::context(&state.cards, None, None);
    rich::prefetch(&mut state.cards, &ctx.client, &rctx, &refs).map(Msg::Rich)
}

/// A rich-content click, as this screen's message.
fn on_rich(action: rich::RichAction) -> Msg {
    Msg::Rich(rich::Ev::Action(action))
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Rich(rich::Ev::Action(action)) => action.route(),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    // One load at a time: a reload asked for meanwhile runs once the three answers are in.
    if state.pending > 0 {
        state.again = true;
        return Task::none();
    }
    state.pending = 3;
    let (c1, c2, c3) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    Task::batch([
        client::task(move || c1.get::<Value>("/v1/approvals"), Msg::Approvals),
        client::task(
            move || c2.get::<Value>("/v1/agent-proposals"),
            Msg::Proposals,
        ),
        client::task(move || c3.get::<Value>("/v1/agents"), Msg::Agents),
    ])
}

/// One of the three load answers arrived; the last one starts a reload that was asked for meanwhile.
fn settled(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.pending = state.pending.saturating_sub(1);
    let next = if state.pending == 0 && std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    };
    Task::batch([next, prefetch(state, ctx)])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Rich(ev) => {
            rich::apply(&mut state.cards, ev);
            Task::none()
        }
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
            state.proposals = res
                .map(|v| list(&v, Proposal::from_value))
                .unwrap_or_default();
            settled(state, ctx)
        }
        Msg::Agents(res) => {
            if let Ok(v) = res {
                state.roster = v.as_array().cloned().unwrap_or_default();
            }
            settled(state, ctx)
        }
        Msg::Event(event) => match event.kind.as_str() {
            "approval" | "agent_proposal" => load(state, ctx),
            _ => Task::none(),
        },
        Msg::Edit(key, action) => {
            state.reasons.entry(key).or_default().perform(action);
            Task::none()
        }
        Msg::Select(id) => {
            state.selected = Some(id);
            Task::none()
        }
        Msg::Decide(id, approve, action) => {
            state.busy.insert(id.clone());
            let mut body = json!({ "approve": approve });
            if let Some(action) = action {
                body["action"] = json!(action);
            }
            let reason = reason_of(state, &format!("ap-{id}"));
            if !reason.is_empty() {
                body["reason"] = json!(reason);
            }
            let client = ctx.client.clone();
            let path = format!("/v1/approvals/{id}");
            client::task(
                move || client.post::<Value>(&path, &body),
                move |r| Msg::Decided(id.clone(), r.map(|_| ())),
            )
        }
        Msg::Decided(id, res) => {
            state.busy.remove(&id);
            match res {
                Ok(()) => {
                    state.reasons.remove(&format!("ap-{id}"));
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(state, ctx)
        }
        Msg::RejectOpen(id) => {
            // Each opening of the dialog asks afresh, as the web's does.
            state.reasons.insert(format!("pp-{id}"), Content::new());
            state.rejecting = Some(id);
            Task::none()
        }
        Msg::RejectCancel => {
            if let Some(id) = state.rejecting.take() {
                state.reasons.remove(&format!("pp-{id}"));
            }
            Task::none()
        }
        Msg::RejectConfirm(id) => {
            let reason = reason_of(state, &format!("pp-{id}"));
            // A proposal's rejection needs a reason (the wizard's rule): with none, nothing is sent and the dialog stays.
            if reason.is_empty() {
                return Task::none();
            }
            state.rejecting = None;
            let client = ctx.client.clone();
            let path = format!("/v1/agent-proposals/{id}/reject");
            let body = json!({ "reason": reason });
            client::task(
                move || client.post::<Value>(&path, &body),
                move |r| Msg::Rejected(id.clone(), r.map(|_| ())),
            )
        }
        Msg::Rejected(id, res) => {
            match res {
                Ok(()) => {
                    state.reasons.remove(&format!("pp-{id}"));
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(state, ctx)
        }
        Msg::ToggleProposalHistory => {
            state.proposals_open = !state.proposals_open;
            Task::none()
        }
        Msg::ToggleHistory => {
            state.history_open = !state.history_open;
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

/// Whether an approval shows under `scope`: under a project, the ones its agents asked for and the orchestrator's (it
/// works on every project), and the plaza's own (cid 0).
fn in_scope(state: &State, cid: u64, scope: Option<&str>) -> bool {
    if scope.is_none() || cid == 0 {
        return true;
    }
    match state.roster.iter().find(|a| a["cid"].as_u64() == Some(cid)) {
        Some(agent) => scope::in_scope(agent, scope),
        None => true,
    }
}

/// The waiting approvals under `scope`, the oldest first.
fn waiting<'a>(state: &'a State, scope: Option<&str>) -> Vec<&'a Approval> {
    let mut waiting: Vec<&Approval> = state
        .approvals
        .iter()
        .filter(|a| a.state == "pending" && in_scope(state, a.cid, scope))
        .collect();
    waiting.sort_by_key(|a| a.created_ms);
    waiting
}

/// What the list shows and the card on the right can be: the waiting proposals, then the waiting approvals.
#[derive(Clone, Copy)]
enum Open<'a> {
    Approval(&'a Approval),
    Proposal(&'a Proposal),
}

impl<'a> Open<'a> {
    fn id(&self) -> &'a str {
        match *self {
            Open::Approval(a) => &a.id,
            Open::Proposal(p) => &p.id,
        }
    }
}

fn open_items<'a>(state: &'a State, scope: Option<&str>) -> Vec<Open<'a>> {
    let mut proposals: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.state == "pending")
        .collect();
    proposals.sort_by_key(|p| p.created_ms);
    let mut out: Vec<Open<'a>> = proposals.into_iter().map(Open::Proposal).collect();
    out.extend(waiting(state, scope).into_iter().map(Open::Approval));
    out
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let rctx = rich::context(&state.cards, None, None);
    let mut layers = Stack::new().push(page(state, &rctx, ctx.project.as_deref()));
    if let Some(id) = state.rejecting.as_deref() {
        layers = layers.push(reject_dialog(state, id));
    }
    if state.wizard.open {
        layers = layers.push(wizard::view(&state.wizard, ctx).map(Msg::Wizard));
    }
    layers.into()
}

fn page<'a>(state: &'a State, rctx: &rich::RichCtx, scope: Option<&str>) -> Element<'a, Msg> {
    if !state.loaded {
        return Space::new().into();
    }
    let open = open_items(state, scope);
    let selected = state.selected.as_deref();
    let current: Option<Open<'a>> = open
        .iter()
        .find(|item| Some(item.id()) == selected)
        .or(open.first())
        .copied();
    let list = list_pane(state, scope, current.map(|item| item.id()));
    let detail: Element<'a, Msg> = match current {
        Some(Open::Approval(a)) => column_pane(waiting_card(state, a)),
        Some(Open::Proposal(p)) => column_pane(proposal_card(state, p, rctx)),
        None => container(ui::empty(
            "shield-check",
            "Nothing is waiting",
            "Agents ask here before acts that need your approval, and the orchestrator proposes new agents here.",
        ))
        .width(Length::Fill)
        .into(),
    };
    let inbox = row![
        container(ui::scroll(list).height(Length::Fill))
            .width(Length::Fixed(380.0))
            .height(Length::Fill),
        container(Space::new().width(1.0))
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(theme::palette().rule.into()),
                ..Default::default()
            }),
        container(ui::scroll(detail).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill);
    match &state.error {
        Some(error) => column![ui::error_line(error), inbox]
            .spacing(12)
            .width(Length::Fill)
            .into(),
        None => inbox.into(),
    }
}

/// The detail column: centred, at most 780 wide, padded as the web's `.inbox-detail .col`.
fn column_pane<'a>(content: Element<'a, Msg>) -> Element<'a, Msg> {
    container(
        container(content)
            .max_width(780.0)
            .width(Length::Fill)
            .padding(Padding {
                top: 24.0,
                right: 28.0,
                bottom: 24.0,
                left: 28.0,
            }),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

/// The list on the left: the proposals waiting, the approvals waiting, then the two decided lists folded.
fn list_pane<'a>(state: &'a State, scope: Option<&str>, current: Option<&str>) -> Element<'a, Msg> {
    let mut pending: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.state == "pending")
        .collect();
    pending.sort_by_key(|p| p.created_ms);
    let mut past: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.state != "pending")
        .collect();
    past.sort_by_key(|p| std::cmp::Reverse(p.decided_ms.max(p.created_ms)));
    let mut history: Vec<&Approval> = state
        .approvals
        .iter()
        .filter(|a| a.state != "pending" && in_scope(state, a.cid, scope))
        .collect();
    history.sort_by_key(|a| std::cmp::Reverse(a.created_ms));

    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if !state.proposals.is_empty() {
        items.push(section("New agents proposed by the orchestrator", 0.0));
        if pending.is_empty() {
            items.push(note("No proposals waiting."));
        } else {
            items.extend(pending.into_iter().map(|p| {
                let title = if p.title.is_empty() {
                    p.id.clone()
                } else {
                    p.title.clone()
                };
                list_item(
                    "user-plus",
                    "proposal",
                    title,
                    label_of(&state.roster, p.by_cid),
                    p.created_ms,
                    current == Some(p.id.as_str()),
                    Msg::Select(p.id.clone()),
                )
            }));
        }
        items.push(section("Waiting", 12.0));
    }
    if state.failed {
        items.push(
            container(ui::sans(
                "Could not load approvals.",
                crate::theme::TYPE_META,
                theme::palette().crit,
            ))
            .padding(pad(16.0, 16.0))
            .into(),
        );
    } else {
        items.extend(waiting(state, scope).into_iter().map(|a| {
            list_item(
                kind_icon(&a.kind),
                &a.kind,
                wait_title(a),
                label_of(&state.roster, a.cid),
                a.created_ms,
                current == Some(a.id.as_str()),
                Msg::Select(a.id.clone()),
            )
        }));
    }
    if !past.is_empty() {
        items.push(fold(
            format!("Decided proposals ({})", past.len()),
            Msg::ToggleProposalHistory,
        ));
        if state.proposals_open {
            items.push(ruled(past.into_iter().map(proposal_row).collect()));
        }
    }
    items.push(fold(
        format!("Decided ({})", history.len()),
        Msg::ToggleHistory,
    ));
    if state.history_open {
        if history.is_empty() {
            items.push(note("No decisions yet."));
        } else {
            items.push(ruled(
                history.into_iter().map(|a| history_row(state, a)).collect(),
            ));
        }
    }
    container(column(items).spacing(2).width(Length::Fill))
        .padding(Padding {
            top: 24.0,
            right: 8.0,
            bottom: 8.0,
            left: 8.0,
        })
        .width(Length::Fill)
        .into()
}

/// One decided approval, as a line of the history.
fn history_row<'a>(state: &'a State, a: &'a Approval) -> Element<'a, Msg> {
    let (label, tone) = match a.state.as_str() {
        "approved" => ("Approved", ui::Tone::Ok),
        "denied" => ("Denied", ui::Tone::Danger),
        "used" => ("Used", ui::Tone::Info),
        "withdrawn" => ("Withdrawn", ui::Tone::Muted),
        _ => ("Waiting", ui::Tone::Warn),
    };
    let mut main = column![
        ui::mono(
            if a.subject.is_empty() {
                a.tool.clone()
            } else {
                a.subject.clone()
            },
            crate::theme::TYPE_CODE,
            theme::palette().ink,
        ),
        ui::text_link(&label_of(&state.roster, a.cid), Msg::Go(Id::Agent(a.cid))),
    ]
    .spacing(4)
    .width(Length::Fill);
    if !a.reason.is_empty() {
        main = main.push(ui::faint(a.reason.clone()));
    }
    row![ui::badge(label, tone), main, ui::faint(ago(a.created_ms)),]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding(pad(10.0, 10.0))
        .width(Length::Fill)
        .into()
}

/// One decided proposal, as a line of the collapsed history.
fn proposal_row<'a>(p: &'a Proposal) -> Element<'a, Msg> {
    let (label, tone) = match p.state.as_str() {
        "approved" => ("Approved", ui::Tone::Ok),
        "rejected" => ("Rejected", ui::Tone::Danger),
        _ => ("Waiting", ui::Tone::Warn),
    };
    let title = if p.title.is_empty() {
        p.id.clone()
    } else {
        p.title.clone()
    };
    let mut main =
        column![ui::sans(title, crate::theme::TYPE_UI, theme::palette().ink).font(SANS_SEMIBOLD)]
            .spacing(4)
            .width(Length::Fill);
    if !p.note.is_empty() {
        main = main.push(ui::faint(p.note.clone()));
    }
    row![
        ui::badge(label, tone),
        main,
        ui::faint(ago(if p.decided_ms != 0 {
            p.decided_ms
        } else {
            p.created_ms
        })),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .padding(pad(10.0, 10.0))
    .width(Length::Fill)
    .into()
}

/// A waiting approval's card: a review with its calls, or a plain card (a tool call or a share).
fn waiting_card<'a>(state: &'a State, a: &'a Approval) -> Element<'a, Msg> {
    let busy = state.busy.contains(&a.id);
    let key = format!("ap-{}", a.id);
    let decide = move |approve: bool, action: Option<&'static str>| {
        (!busy).then(|| Msg::Decide(a.id.clone(), approve, action))
    };
    let agent = ui::text_link(&label_of(&state.roster, a.cid), Msg::Go(Id::Agent(a.cid)));
    if a.kind == "review" && is_review(a) {
        return review_card(state, a, &key, busy);
    }
    let deny = ui::btn("Deny", None, ui::Kind::Danger, decide(false, None));
    let approve = ui::btn("Approve", None, ui::Kind::Primary, decide(true, None));
    let (chips, body): (Vec<Element<'a, Msg>>, Element<'a, Msg>) = match a.kind.as_str() {
        "artifact.share" => (
            vec![agent, ui::badge("Share", ui::Tone::Signal)],
            share_body(&a.detail),
        ),
        _ => (
            vec![
                agent,
                bordered(
                    ui::mono(
                        a.tool.clone(),
                        crate::theme::TYPE_CODE,
                        theme::palette().ink,
                    )
                    .into(),
                    pad(0.0, 4.0),
                ),
            ],
            json_box(&a.input),
        ),
    };
    card(
        card_head(
            tile(kind_icon(&a.kind), &a.kind, 36.0, 18.0, 10.0),
            wait_title(a),
            chips,
            a.created_ms,
        ),
        body,
        card_foot(
            reason_box(state, &key, busy, "Reason (optional)", 56.0),
            vec![deny, approve],
        ),
    )
}

/// The review's own check: a review that names its run and its task.
fn is_review(a: &Approval) -> bool {
    !field(&a.detail, "run").is_empty() && !field(&a.detail, "task").is_empty()
}

/// The JSON of a tool's input, on sheet-2 and scrolling past 280 (the web's `.ap-pre`).
fn json_box<'a>(value: &Value) -> Element<'a, Msg> {
    let source = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string());
    container(ui::scroll(ui::mono(
        source,
        crate::theme::TYPE_CODE,
        theme::palette().ink,
    )))
    .padding(pad(12.0, 12.0))
    .width(Length::Fill)
    .max_height(280.0)
    .style(|_: &Theme| container::Style {
        background: Some(theme::palette().sheet_2.into()),
        border: Border::default().rounded(theme::skin().radius),
        ..Default::default()
    })
    .into()
}

/// A share request: the reason the agent gives, and the options it asks for as chips.
fn share_body<'a>(detail: &Value) -> Element<'a, Msg> {
    let options = detail.get("options");
    let expiry = match options
        .and_then(|o| o.get("expires_in_days"))
        .and_then(Value::as_u64)
        .filter(|days| *days > 0)
    {
        Some(1) => "Expires in 1 day".to_string(),
        Some(days) => format!("Expires in {days} days"),
        None => "Never expires".to_string(),
    };
    let downloads = if options
        .and_then(|o| o.get("allow_download"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "Downloading allowed"
    } else {
        "View only"
    };
    let version = match options
        .and_then(|o| o.get("version"))
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
    {
        Some(v) => format!("Pinned to v{v}"),
        None => "Follows the latest version".to_string(),
    };
    let reason = field(detail, "reason");
    let mut body = column![].spacing(12);
    if !reason.is_empty() {
        body = body.push(ui::sans(
            reason,
            crate::theme::TYPE_PROSE,
            theme::palette().ink_2,
        ));
    }
    let chips: Vec<Element<'a, Msg>> = [expiry, downloads.to_string(), version]
        .into_iter()
        .map(|label| {
            bordered(
                ui::sans(label, crate::theme::TYPE_UI, theme::palette().ink).into(),
                pad(4.0, 8.0),
            )
        })
        .collect();
    body.push(ui::chip_row(chips)).into()
}

/// A review the owner decides: its verdict, what needs the owner, the readings as level dots, the meta line, and the
/// three calls (Fail, Send back, Pass). The calls post `{approve, action}` like any approval.
fn review_card<'a>(state: &'a State, a: &'a Approval, key: &str, busy: bool) -> Element<'a, Msg> {
    let d = &a.detail;
    let decide = move |approve: bool, action: Option<&'static str>| {
        (!busy).then(|| Msg::Decide(a.id.clone(), approve, action))
    };
    let goal = field(d, "goal");
    let title = if goal.is_empty() {
        format!("Review of {}", field(d, "task"))
    } else {
        goal
    };
    let worker = label_of(&state.roster, a.cid);
    let mut meta = vec![format!("Worker: {worker}")];
    let change = field(d, "change");
    if !change.is_empty() {
        meta.push(format!("change {change}"));
    }
    if let Some(round) = d.get("round").and_then(Value::as_u64).filter(|r| *r > 0) {
        meta.push(format!("round {round}"));
    }
    let readings: Vec<Element<'a, Msg>> = d
        .get("readings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            let level = r.get("level").and_then(Value::as_i64);
            bordered(
                row![
                    ui::dot(reading_tone(level).color()),
                    ui::sans(
                        field(r, "name"),
                        crate::theme::TYPE_UI,
                        theme::palette().ink
                    ),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
                pad(4.0, 8.0),
            )
        })
        .collect();

    let mut body = column![].spacing(12);
    let verdict = field(d, "verdict");
    if !verdict.is_empty() {
        body = body.push(ui::sans(
            verdict,
            crate::theme::TYPE_UI,
            theme::palette().ink_2,
        ));
    }
    let needs = field(d, "needs_you");
    if !needs.is_empty() {
        body = body.push(
            row![
                ui::sans("Needs you: ", crate::theme::TYPE_UI, theme::palette().ink_2)
                    .font(SANS_SEMIBOLD),
                ui::sans(needs, crate::theme::TYPE_UI, theme::palette().ink_2).width(Length::Fill),
            ]
            .width(Length::Fill),
        );
    }
    if !readings.is_empty() {
        body = body.push(ui::chip_row(readings));
    }
    body = body.push(ui::faint(meta.join(" · ")));

    let calls = vec![
        ui::btn("Fail", None, ui::Kind::Danger, decide(false, None)),
        ui::btn(
            "Send back",
            None,
            ui::Kind::Secondary,
            decide(false, Some("rework")),
        ),
        ui::btn("Pass", None, ui::Kind::Primary, decide(true, None)),
    ];
    card(
        card_head(
            tile(kind_icon("review"), "review", 36.0, 18.0, 10.0),
            title,
            vec![
                ui::text_link(&label_of(&state.roster, a.cid), Msg::Go(Id::Agent(a.cid))),
                ui::badge("Review", ui::Tone::Signal),
            ],
            a.created_ms,
        ),
        body.into(),
        card_foot(
            reason_box(state, key, busy, "Reason (optional)", 56.0),
            calls,
        ),
    )
}

/// A reading's tone as the web's `readingDot` draws it: better is ok, worse is danger, neutral or not measured is muted.
fn reading_tone(level: Option<i64>) -> ui::Tone {
    match level {
        Some(l) if l > 0 => ui::Tone::Ok,
        Some(l) if l < 0 => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// The card of one proposed agent, waiting: what it is for, why the orchestrator wants it, and the owner's call.
fn proposal_card<'a>(state: &'a State, p: &'a Proposal, rctx: &rich::RichCtx) -> Element<'a, Msg> {
    let title = if p.title.is_empty() {
        p.id.clone()
    } else {
        p.title.clone()
    };
    let mut chips: Vec<Element<'a, Msg>> = Vec::new();
    if p.new_role {
        chips.push(ui::badge("new role", ui::Tone::Signal));
    }
    chips.push(ui::badge(
        &format!(
            "{} {}",
            p.count,
            if p.count == 1 { "agent" } else { "agents" }
        ),
        ui::Tone::Info,
    ));
    let mut body = column![rich::view(&p.reason, rctx, &state.cards, &on_rich)].spacing(12);
    if !p.instruction.is_empty() {
        body = body.push(ground(
            rich::view(
                &format!("“{}”", p.instruction),
                rctx,
                &state.cards,
                &on_rich,
            ),
            pad(8.0, 12.0),
            8.0,
        ));
    }
    body = body.push(ui::sans(
        format!("Proposed by {}", label_of(&state.roster, p.by_cid)),
        crate::theme::TYPE_UI,
        theme::palette().ink_2,
    ));
    // The review opens the new-agent wizard on this proposal (approve or reject there).
    let reject = ui::btn(
        "Reject",
        None,
        ui::Kind::Danger,
        Some(Msg::RejectOpen(p.id.clone())),
    );
    let review = ui::btn(
        "Review…",
        None,
        ui::Kind::Primary,
        p.review
            .clone()
            .map(|v| Msg::Wizard(WizardMsg::OpenProposal(v))),
    );
    card(
        card_head(
            tile(kind_icon("proposal"), "proposal", 36.0, 18.0, 10.0),
            title,
            chips,
            p.created_ms,
        ),
        body.into(),
        card_foot(Space::new().into(), vec![reject, review]),
    )
}

/// The dialog that asks a proposal's rejection for its reason: a sheet over a scrim, a field, Cancel and Reject. A
/// blank reason keeps it open, and the Reject button stays off until one is typed.
fn reject_dialog<'a>(state: &'a State, id: &'a str) -> Element<'a, Msg> {
    let title = state
        .proposals
        .iter()
        .find(|p| p.id == id)
        .map(|p| {
            if p.title.is_empty() {
                p.id.clone()
            } else {
                p.title.clone()
            }
        })
        .unwrap_or_default();
    let key = format!("pp-{id}");
    let ready = !reason_of(state, &key).is_empty();
    let content = state.reasons.get(&key).unwrap_or(&state.blank);
    let editor_field = {
        let key = key.clone();
        text_editor(content)
            .height(Length::Fixed(96.0))
            .style(editor)
            .on_action(move |action| Msg::Edit(key.clone(), action))
    };
    let head = container(
        row![
            ui::sans(
                format!("Reject {title}? Give a reason."),
                17.0,
                theme::palette().ink
            )
            .font(SANS_SEMIBOLD)
            .width(Length::Fill),
            ui::btn("", Some("x"), ui::Kind::Ghost, Some(Msg::RejectCancel)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 16.0,
        right: 14.0,
        bottom: 6.0,
        left: 22.0,
    });
    let body = container(editor_field).padding(Padding {
        top: 8.0,
        right: 22.0,
        bottom: 16.0,
        left: 22.0,
    });
    let foot = column![
        rule(),
        container(
            row![
                Space::new().width(Length::Fill),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::RejectCancel)),
                ui::btn(
                    "Reject",
                    None,
                    ui::Kind::Danger,
                    ready.then(|| Msg::RejectConfirm(id.to_string())),
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            top: 12.0,
            right: 22.0,
            bottom: 16.0,
            left: 22.0,
        }),
    ];
    let panel = container(column![head, body, foot])
        .width(Length::Fixed(560.0))
        .style(theme::pop);
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(theme::scrim)
        .into()
}

/// The hub events this screen follows live (the web refreshes on the same topics).
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(kind: &str, detail: Value, input: Value) -> Approval {
        Approval {
            id: "ap-1".into(),
            cid: 3,
            kind: kind.into(),
            subject: "lab_dataset".into(),
            tool: "shell".into(),
            input,
            detail,
            created_ms: 0,
            state: "pending".into(),
            reason: String::new(),
        }
    }

    #[test]
    fn a_card_is_titled_as_the_web_titles_it() {
        let review = approval(
            "review",
            json!({ "goal": "Ship 0.9", "task": "t1", "run": "r1" }),
            Value::Null,
        );
        assert_eq!(wait_title(&review), "Ship 0.9");
        let bare = approval("review", json!({ "task": "t1", "run": "r1" }), Value::Null);
        assert_eq!(wait_title(&bare), "Review of t1");
        assert!(is_review(&bare));
        let tool = approval("tool", Value::Null, json!({ "command": "pnpm add x" }));
        assert_eq!(wait_title(&tool), "shell: pnpm add x");
        assert_eq!(
            wait_title(&approval("tool", Value::Null, Value::Null)),
            "shell"
        );
    }

    #[test]
    fn the_readings_are_ok_above_zero_and_danger_below() {
        assert_eq!(reading_tone(Some(2)), ui::Tone::Ok);
        assert_eq!(reading_tone(Some(-1)), ui::Tone::Danger);
        assert_eq!(reading_tone(Some(0)), ui::Tone::Muted);
        assert_eq!(reading_tone(None), ui::Tone::Muted);
    }
}
