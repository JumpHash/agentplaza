//! The orchestrator's proposal, reviewed in a modal over the Approvals screen (the web's proposal review): its three
//! steps are read-only (the proposal and its reason, the role it defines, the count and the model), and the last one
//! approves it (the agents start) or rejects it with a reason. Spawning by hand is gone: a session starts its seat's work.

use iced::widget::text_editor::Content;
use iced::widget::{column, container, pick_list, row, text, text_editor};
use iced::{Background, Border, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::console::Ctx;
use crate::console::client;
use crate::console::picker::{self, Choice, PickerMsg};
use crate::console::ui::{self, Kind};
use crate::theme::{
    SANS_MEDIUM, SANS_SEMIBOLD, TYPE_ITEM, TYPE_META, TYPE_SECTION, TYPE_UI, palette, skin,
};

const PROPOSAL_STEPS: [&str; 3] = ["The proposal", "The role", "Decide"];
const EFFORTS: [&str; 7] = ["none", "low", "medium", "high", "xhigh", "max", "ultra"];
const MAX_AGENTS: u32 = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    Existing,
    #[default]
    New,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoleDefView {
    pub title: String,
    pub purpose: String,
    pub responsibilities: Vec<String>,
    pub deliverables: Vec<String>,
    pub collaboration: String,
    pub boundaries: Vec<String>,
    pub tools: Vec<String>,
    pub budget: i64,
    pub model: String,
    /// The role's prompt as the hub holds it (the wizard starts its editor from it).
    pub prompt: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RoleView {
    pub id: String,
    pub title: String,
    pub source: String,
    pub def: RoleDefView,
    pub agents: u32,
}

/// An orchestrator's proposal for a new agent (`/v1/agent-proposals`), reviewed here as the web's wizard does.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProposalView {
    pub id: String,
    /// The proposed role, as a `RoleDef` (its `id` included).
    pub role: Value,
    pub new_role: bool,
    pub count: u32,
    pub model: String,
    pub reason: String,
    pub instruction: String,
}

impl ProposalView {
    fn role_id(&self) -> String {
        self.role
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn role_def(&self) -> RoleDefView {
        serde_json::from_value(self.role.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ModelPolicy {
    worker: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SettingsView {
    default_model: String,
    model_policy: ModelPolicy,
}

/// What the wizard loads when it opens: roles, settings and the model list.
#[derive(Debug, Clone)]
pub struct Loaded {
    roles: Vec<RoleView>,
    settings: SettingsView,
    models: Value,
}

#[derive(Debug, Default)]
pub struct Wizard {
    pub open: bool,
    pub error: Option<String>,
    step: usize,
    busy: bool,
    loaded: bool,
    load_error: Option<String>,
    roles: Vec<RoleView>,
    worker_policy: Vec<String>,
    models: Value,
    choices: Vec<Choice>,
    picker: picker::Picker,
    mode: Mode,
    role_id: String,
    id: String,
    id_touched: bool,
    title: String,
    purpose: Content,
    collaboration: Content,
    responsibilities: Vec<String>,
    deliverables: Vec<String>,
    boundaries: Vec<String>,
    tools: Vec<String>,
    budget_text: String,
    budget: Option<i64>,
    /// Bare `provider/model`.
    model: String,
    effort: String,
    count: u32,
    /// The prompt as the user edited it; None means "generated from the fields".
    prompt_edited: Option<String>,
    editing_prompt: bool,
    /// The orchestrator's proposal under review; the last step approves or rejects it instead of spawning.
    proposal: Option<ProposalView>,
    rejecting: bool,
    /// The rejection reason, typed in a multi-line editor (Enter adds a line, as the web's textarea).
    reject_reason: Content,
}

#[derive(Debug, Clone)]
pub enum WizardMsg {
    Close,
    /// Spawned (or failed): the Agents screen reloads.
    Done(Result<(), String>),
    Loaded(Result<Loaded, String>),
    Retry,
    Go(usize),
    Picker(PickerMsg),
    /// The owner's favourites after a star in the picker.
    Favourited(Result<Vec<String>, String>),
    Count(u32),
    Approve,
    /// Opens the wizard on an orchestrator's proposal (the approvals screen sends it).
    OpenProposal(ProposalView),
    RejectToggle,
    RejectEdit(text_editor::Action),
    Reject,
}

impl Wizard {
    fn fresh() -> Wizard {
        Wizard {
            budget_text: "2000".into(),
            budget: Some(2000),
            count: 1,
            ..Default::default()
        }
    }

    fn purpose_text(&self) -> String {
        self.purpose.text().trim().to_string()
    }

    fn collaboration_text(&self) -> String {
        self.collaboration.text().trim().to_string()
    }

    fn budget_value(&self) -> i64 {
        self.budget.unwrap_or(0)
    }

    fn generated_prompt(&self) -> String {
        prompt_text(
            &self.title,
            &self.purpose_text(),
            &self.responsibilities,
            &self.deliverables,
            &self.collaboration_text(),
            &self.boundaries,
            self.budget_value(),
        )
    }

    fn current_prompt(&self) -> String {
        self.prompt_edited
            .clone()
            .unwrap_or_else(|| self.generated_prompt())
    }

    /// `provider/model`, with `@effort` when one is chosen.
    fn model_value(&self) -> String {
        if self.model.is_empty() || self.effort.is_empty() {
            self.model.clone()
        } else {
            format!("{}@{}", self.model, self.effort)
        }
    }

    fn refresh_choices(&mut self) {
        self.choices = worker_choices(&self.models, &self.worker_policy, &self.model);
    }

    /// The role definition as the hub stores it (`RoleDef`).
    fn role_json(&self, model: &str) -> Value {
        json!({
            "id": self.id,
            "title": self.title.trim(),
            "purpose": self.purpose_text(),
            "responsibilities": nonempty(&self.responsibilities),
            "deliverables": nonempty(&self.deliverables),
            "collaboration": self.collaboration_text(),
            "boundaries": nonempty(&self.boundaries),
            "tools": self.tools,
            "budget": self.budget_value(),
            "model": model,
            "prompt": self.current_prompt(),
            "created_by": "",
            "created_ms": 0,
        })
    }

    /// Seeds the fields from the proposal's role, as the web's wizard does.
    fn open_proposal(&mut self, p: ProposalView) {
        let def = p.role_def();
        let role_id = p.role_id();
        self.mode = if p.new_role {
            Mode::New
        } else {
            Mode::Existing
        };
        self.role_id = if p.new_role {
            String::new()
        } else {
            role_id.clone()
        };
        self.id = role_id;
        self.id_touched = !self.id.is_empty();
        self.title = def.title.clone();
        self.purpose = Content::with_text(&def.purpose);
        self.collaboration = Content::with_text(&def.collaboration);
        self.responsibilities = def.responsibilities.clone();
        self.deliverables = def.deliverables.clone();
        self.boundaries = def.boundaries.clone();
        self.tools = def.tools.clone();
        let budget = if def.budget > 0 { def.budget } else { 2000 };
        self.budget = Some(budget);
        self.budget_text = budget.to_string();
        let raw = if p.model.is_empty() {
            def.model.clone()
        } else {
            p.model.clone()
        };
        let (base, effort) = split_model(&raw);
        self.model = base;
        self.effort = effort;
        self.count = p.count.clamp(1, MAX_AGENTS);
        self.prompt_edited = Some(def.prompt.clone()).filter(|s| !s.is_empty());
        self.proposal = Some(p);
    }

    /// Approves the proposal with the wizard's edits, as the web's approve does.
    fn approve(&mut self, ctx: &Ctx) -> Task<WizardMsg> {
        let Some(id) = self.proposal.as_ref().map(|p| p.id.clone()) else {
            return Task::none();
        };
        self.busy = true;
        self.error = None;
        let model = self.model_value();
        let count = self.count;
        let edits = match self.mode {
            Mode::New => json!({ "role": self.role_json(&model), "count": count, "model": model }),
            Mode::Existing => json!({ "count": count, "model": model }),
        };
        let client = ctx.client.clone();
        client::task(
            move || {
                client.post::<Value>(&format!("/v1/agent-proposals/{id}/approve"), &edits)?;
                Ok(())
            },
            WizardMsg::Done,
        )
    }

    /// Rejects the proposal with the owner's reason; the reject button stays off until one is typed.
    fn reject(&mut self, ctx: &Ctx) -> Task<WizardMsg> {
        let reason = self.reject_reason.text().trim().to_string();
        let Some(id) = self.proposal.as_ref().map(|p| p.id.clone()) else {
            return Task::none();
        };
        if reason.is_empty() {
            return Task::none();
        }
        self.busy = true;
        self.error = None;
        let client = ctx.client.clone();
        client::task(
            move || {
                client.post::<Value>(
                    &format!("/v1/agent-proposals/{id}/reject"),
                    &json!({ "reason": reason }),
                )?;
                Ok(())
            },
            WizardMsg::Done,
        )
    }
}

/// Opens the wizard's data load: roles, settings and models.
fn load(ctx: &Ctx) -> Task<WizardMsg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            let roles: Vec<RoleView> = client.get("/v1/roles")?;
            let settings: SettingsView = client.get("/v1/settings")?;
            let models: Value = client.get("/v1/models")?;
            Ok(Loaded {
                roles,
                settings,
                models,
            })
        },
        WizardMsg::Loaded,
    )
}

pub fn update(wizard: &mut Wizard, msg: WizardMsg, ctx: &Ctx) -> Task<WizardMsg> {
    match msg {
        WizardMsg::Close => wizard.open = false,
        WizardMsg::Done(Ok(())) => {
            wizard.busy = false;
            wizard.open = false;
        }
        WizardMsg::Done(Err(e)) => {
            wizard.busy = false;
            wizard.error = Some(e);
        }
        WizardMsg::Loaded(Ok(loaded)) => {
            let fallback = split_model(&loaded.settings.default_model).0;
            wizard.roles = loaded.roles;
            if let Some(p) = wizard.proposal.clone().filter(|p| !p.new_role) {
                let id = p.role_id();
                if !wizard.roles.iter().any(|r| r.id == id) {
                    let def = p.role_def();
                    wizard.roles.push(RoleView {
                        id,
                        title: def.title.clone(),
                        source: String::new(),
                        def,
                        agents: 0,
                    });
                }
            }
            wizard.worker_policy = loaded.settings.model_policy.worker;
            wizard.models = loaded.models;
            picker::sync(&mut wizard.picker, &wizard.models);
            if wizard.model.is_empty() {
                wizard.model = fallback;
            }
            wizard.refresh_choices();
            if wizard.model.is_empty()
                && let Some(first) = wizard
                    .choices
                    .iter()
                    .find(|c| !picker::trains_on_data(&wizard.models, &c.id))
            {
                wizard.model = first.id.clone();
            }
            wizard.loaded = true;
            wizard.load_error = None;
        }
        WizardMsg::Loaded(Err(e)) => wizard.load_error = Some(e),
        WizardMsg::Retry => {
            wizard.load_error = None;
            return load(ctx);
        }
        WizardMsg::Go(to) => {
            // The proposal's steps are read-only, so they always move (as the web's do).
            let count = step_names().len();
            if to < count {
                wizard.step = to;
                wizard.editing_prompt = false;
                wizard.rejecting = false;
            }
        }
        WizardMsg::Picker(m) => {
            picker::update(&mut wizard.picker, &m);
            match m {
                PickerMsg::Pick(id) => wizard.model = split_model(&id).0,
                PickerMsg::Effort(effort) => wizard.effort = effort,
                PickerMsg::Favourite(id) => {
                    let c = ctx.client.clone();
                    return client::task(
                        move || picker::save_favourite(&c, &id),
                        WizardMsg::Favourited,
                    );
                }
                _ => {}
            }
        }
        WizardMsg::Favourited(Ok(favourites)) => {
            picker::mark_favourites(&mut wizard.models, &favourites);
            picker::sync(&mut wizard.picker, &wizard.models);
        }
        WizardMsg::Favourited(Err(e)) => wizard.error = Some(e),
        WizardMsg::Count(n) => wizard.count = n.clamp(1, MAX_AGENTS),
        WizardMsg::Approve => {
            if !wizard.busy {
                return wizard.approve(ctx);
            }
        }
        WizardMsg::OpenProposal(p) => {
            *wizard = Wizard::fresh();
            wizard.open = true;
            wizard.open_proposal(p);
            return load(ctx);
        }
        WizardMsg::RejectToggle => {
            // "Keep it" closes the form and drops the typed reason, as the web's does.
            if wizard.rejecting {
                wizard.reject_reason = Content::new();
            }
            wizard.rejecting = !wizard.rejecting;
        }
        WizardMsg::RejectEdit(action) => wizard.reject_reason.perform(action),
        WizardMsg::Reject => {
            if !wizard.busy {
                return wizard.reject(ctx);
            }
        }
    }
    Task::none()
}

/// The modal (the caller stacks it over its screen while `open`): the web's `modal()`, with the stepper and the step
/// above the foot, and the navigation in the foot.
pub fn view<'a>(wizard: &'a Wizard, ctx: &'a Ctx) -> Element<'a, WizardMsg> {
    let _ = ctx;
    let mut body = column![stepper(wizard, step_names()), body_host(wizard)].spacing(14);
    if let Some(e) = &wizard.error {
        body = body.push(ui::error_line(e));
    }
    ui::modal(
        "Review the orchestrator's proposal",
        body.into(),
        nav(wizard),
        WizardMsg::Close,
        560.0,
    )
}

/// The proposal's steps: the proposal and its reason, the role it defines, then the decision.
fn step_names() -> &'static [&'static str] {
    &PROPOSAL_STEPS
}

/// The stepper (`ol.ag-wiz-steps`): the steps passed are ghost buttons back to them, the current one sits on the
/// signal ground, the rest are muted.
fn stepper<'a>(w: &'a Wizard, names: &[&str]) -> Element<'a, WizardMsg> {
    let p = palette();
    let (ink, ink_3, soft) = (p.ink, p.ink_3, p.signal_soft);
    row(names.iter().enumerate().map(|(i, name)| {
        let label = format!("{}. {name}", i + 1);
        if i < w.step {
            ui::btn_sm(&label, None, Kind::Ghost, Some(WizardMsg::Go(i)))
        } else {
            let current = i == w.step;
            container(ui::sans(label, TYPE_UI, if current { ink } else { ink_3 }))
                .padding([4, 8])
                .style(move |_: &Theme| container::Style {
                    background: current.then_some(Background::Color(soft)),
                    border: Border {
                        radius: skin().radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        }
    }))
    .spacing(8)
    .into()
}

fn nav<'a>(w: &'a Wizard) -> Vec<Element<'a, WizardMsg>> {
    let last = w.step + 1 == step_names().len();
    let back = w.step.checked_sub(1).filter(|_| !w.busy).map(WizardMsg::Go);
    let mut out = vec![ui::btn("Back", None, Kind::Ghost, back)];
    if !last {
        let next = (w.loaded && !w.busy).then_some(WizardMsg::Go(w.step + 1));
        out.push(ui::btn("Next", None, Kind::Primary, next));
    } else if w.rejecting {
        out.push(ui::btn(
            "Keep it",
            None,
            Kind::Ghost,
            Some(WizardMsg::RejectToggle),
        ));
        let send = !w.busy && !w.reject_reason.text().trim().is_empty();
        out.push(ui::btn(
            "Reject",
            None,
            Kind::Danger,
            send.then_some(WizardMsg::Reject),
        ));
    } else {
        out.push(ui::btn(
            "Reject...",
            None,
            Kind::Ghost,
            (!w.busy).then_some(WizardMsg::RejectToggle),
        ));
        out.push(ui::btn(
            "Approve and spawn",
            None,
            Kind::Primary,
            (!w.busy).then_some(WizardMsg::Approve),
        ));
    }
    out
}

/// The step's body while the roles and models load (or after they failed, with a retry).
fn body_host<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    if w.loaded {
        return body_for(w);
    }
    match &w.load_error {
        Some(e) => column![
            ui::sans(
                format!("Could not load roles and models: {e}"),
                TYPE_ITEM,
                palette().ink
            ),
            ui::btn("Retry", None, Kind::Ghost, Some(WizardMsg::Retry)),
        ]
        .spacing(12)
        .into(),
        None => muted("Loading roles…"),
    }
}

fn body_for<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    proposal_body(w)
}

// ---- Text helpers, as the web's classes -------------------------------------------------------------------------

/// A heading (`h3`): semibold, in ink.
fn heading<'a>(s: &str) -> Element<'a, WizardMsg> {
    text(s.to_string())
        .font(SANS_SEMIBOLD)
        .size(TYPE_SECTION)
        .color(palette().ink)
        .into()
}

/// A sub-heading (`h4`).
fn subheading<'a>(s: &str) -> Element<'a, WizardMsg> {
    text(s.to_string())
        .font(SANS_SEMIBOLD)
        .size(TYPE_UI)
        .color(palette().ink)
        .into()
}

/// Muted text (`.muted`).
fn muted<'a>(s: impl Into<String>) -> Element<'a, WizardMsg> {
    ui::sans(s.into(), TYPE_UI, palette().ink_3).into()
}

/// Body text in ink (a paragraph, a read-only value).
fn ink_text<'a>(s: impl Into<String>) -> Element<'a, WizardMsg> {
    ui::sans(s.into(), TYPE_ITEM, palette().ink).into()
}

/// The hint under a field (`.hint`): small, muted.
fn hint<'a>(s: &str) -> Element<'a, WizardMsg> {
    ui::sans(s.to_string(), TYPE_META, palette().ink_3)
        .line_height(1.4)
        .into()
}

/// A read-only pair (the web's `ro()`): the muted label over its value.
fn ro<'a>(label: &str, value: &str) -> Element<'a, WizardMsg> {
    let value = if value.trim().is_empty() {
        "—"
    } else {
        value
    };
    column![muted(label.to_string()), ink_text(value.to_string())]
        .spacing(2)
        .into()
}

/// A read-only list (the web's `ul`): one bullet a line, or a dash when there are none.
fn ro_list<'a>(label: &str, items: &[String]) -> Element<'a, WizardMsg> {
    let xs = nonempty(items);
    let value: Element<'a, WizardMsg> = if xs.is_empty() {
        ink_text("—")
    } else {
        column(xs.into_iter().map(|x| ink_text(format!("• {x}"))))
            .spacing(2)
            .into()
    };
    column![muted(label.to_string()), value].spacing(2).into()
}

/// A form field (`label.field`): the label, the control, and the hint under it.
fn field<'a>(
    label: &str,
    control: Element<'a, WizardMsg>,
    hint_text: &str,
) -> Element<'a, WizardMsg> {
    let mut col = column![
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(12.5)
            .color(palette().ink_2),
        control
    ]
    .spacing(5);
    if !hint_text.is_empty() {
        col = col.push(hint(hint_text));
    }
    col.into()
}

// ---- The spawn wizard's steps -----------------------------------------------------------------------------------

/// The agents' count, 1 to 8.
fn count_pick<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let options: Vec<u32> = (1..=MAX_AGENTS).collect();
    pick_list(options, Some(w.count), WizardMsg::Count)
        .width(Length::Fill)
        .style(crate::theme::pick_list)
        .into()
}

/// The worker model and, for a spawn, its effort (the picker's own selector).
fn model_pick<'a>(w: &'a Wizard, effort: bool) -> Element<'a, WizardMsg> {
    picker::view(
        &w.picker,
        &w.choices,
        &w.model,
        effort.then_some(w.effort.as_str()),
        WizardMsg::Picker,
    )
}

/// The model's label as the picker shows it, or the default.
fn model_label(w: &Wizard) -> String {
    w.choices
        .iter()
        .find(|c| c.id == w.model)
        .map(|c| c.label.clone())
        .unwrap_or_else(|| {
            if w.model.is_empty() {
                "Model default".into()
            } else {
                w.model.clone()
            }
        })
}

// ---- A proposal's three steps -----------------------------------------------------------------------------------

/// The proposal's steps: the proposal and its reason, the role it defines (with the count and the model), then the
/// decision (approve, or reject with a reason).
fn proposal_body<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let Some(p) = &w.proposal else {
        return column![].into();
    };
    let def = p.role_def();
    let rid = p.role_id();
    let title = if !def.title.is_empty() {
        def.title.clone()
    } else if !rid.is_empty() {
        rid.clone()
    } else {
        "Role".to_string()
    };
    match w.step {
        0 => {
            let instruction: Element<'a, WizardMsg> = if p.instruction.is_empty() {
                muted("No instruction was quoted with this proposal.")
            } else {
                container(ink_text(p.instruction.clone()))
                    .padding([0, 14])
                    .into()
            };
            column![
                heading("The orchestrator proposes"),
                ink_text(if p.reason.trim().is_empty() {
                    "—".to_string()
                } else {
                    p.reason.clone()
                }),
                subheading("The owner's instruction"),
                instruction,
            ]
            .spacing(12)
            .into()
        }
        1 => {
            let existing = w.roles.iter().find(|r| r.id == rid);
            let (kind, note) = match existing {
                Some(ex) => (
                    "an existing",
                    format!(
                        "{} already run this role. The proposal adds agents of it.",
                        plural(ex.agents, "agent")
                    ),
                ),
                None => (
                    "a new",
                    "Approving it writes the role into the plaza with this definition.".to_string(),
                ),
            };
            column![
                heading(&title),
                muted(format!("{rid}, {kind} role")),
                hint(&note),
                ro("Purpose", &def.purpose),
                ro_list("Responsibilities", &def.responsibilities),
                ro_list("Deliverables", &def.deliverables),
                ro("Working with others", &def.collaboration),
                ro_list("Boundaries", &def.boundaries),
                field("Agents", count_pick(w), ""),
                field("Model", model_pick(w, false), ""),
            ]
            .spacing(12)
            .into()
        }
        _ => {
            let mut col = column![
                heading("Approve it, or reject it with a reason"),
                muted(format!(
                    "Approving starts {} of {title} on {}.",
                    plural(w.count, "agent"),
                    model_label(w)
                )),
            ]
            .spacing(12);
            if w.rejecting {
                col = col.push(reject_panel(w));
            }
            col.into()
        }
    }
}

/// The reject form: the reason is required, and Reject stays off until one is typed.
fn reject_panel<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let reason = text_editor(&w.reject_reason)
        .style(crate::theme::editor)
        .placeholder("Why the proposal is rejected")
        .height(Length::Fixed(80.0))
        .padding(8)
        .on_action(WizardMsg::RejectEdit);
    column![subheading("Reject it with a reason"), reason]
        .spacing(12)
        .into()
}

fn plural(n: u32, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Splits `provider/model[@effort]`; an effort the wizard does not know is dropped.
fn split_model(value: &str) -> (String, String) {
    match value.split_once('@') {
        Some((base, effort)) => (
            base.to_string(),
            if EFFORTS.contains(&effort) {
                effort.to_string()
            } else {
                String::new()
            },
        ),
        None => (value.to_string(), String::new()),
    }
}

fn provider_of(id: &str) -> String {
    id.split('/').next().unwrap_or("").to_string()
}

/// Worker choices: the policy's models first, then the rest of the list, the current one always offered.
fn worker_choices(models: &Value, policy: &[String], current: &str) -> Vec<Choice> {
    let all = picker::choices(models);
    let mut out: Vec<Choice> = Vec::new();
    for value in policy {
        let base = split_model(value).0;
        if base.is_empty() || out.iter().any(|c| c.id == base) {
            continue;
        }
        let choice = all
            .iter()
            .find(|c| c.id == base)
            .cloned()
            .unwrap_or_else(|| Choice {
                id: base.clone(),
                provider: provider_of(&base),
                label: base.clone(),
            });
        out.push(choice);
    }
    for c in all {
        if !out.iter().any(|o| o.id == c.id) {
            out.push(c);
        }
    }
    if !current.is_empty() && !out.iter().any(|c| c.id == current) {
        out.push(Choice {
            id: current.to_string(),
            provider: provider_of(current),
            label: current.to_string(),
        });
    }
    out
}

/// The role prompt, in the sections the server reads (# Title, ## Purpose, … ## Budget).
fn prompt_text(
    title: &str,
    purpose: &str,
    responsibilities: &[String],
    deliverables: &[String],
    collaboration: &str,
    boundaries: &[String],
    budget: i64,
) -> String {
    let bullets = |xs: &[String]| {
        nonempty(xs)
            .iter()
            .map(|x| format!("- {x}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let sections = [
        ("Purpose", purpose.trim().to_string()),
        ("Responsibilities", bullets(responsibilities)),
        ("Deliverables", bullets(deliverables)),
        ("Working with others", collaboration.trim().to_string()),
        ("Boundaries", bullets(boundaries)),
        ("Budget", format!("Default task budget: {budget}.")),
    ];
    let mut out = format!("# {}", title.trim());
    for (name, text) in sections {
        if !text.is_empty() {
            out.push_str(&format!("\n\n## {name}\n{text}"));
        }
    }
    out.push('\n');
    out
}

fn nonempty(xs: &[String]) -> Vec<String> {
    xs.iter()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}
