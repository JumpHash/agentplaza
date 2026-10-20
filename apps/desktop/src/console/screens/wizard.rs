//! The new-agent wizard (`agent-wizard.ts`): define a role (or pick one), its model and effort,
//! its tools and its prompt, then spawn — a modal over the Agents screen.

use iced::widget::text_editor::Content;
use iced::widget::{
    button, checkbox, column, container, pick_list, row, text, text_editor, text_input,
};
use iced::{Alignment, Background, Border, Element, Length, Shadow, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::console::Ctx;
use crate::console::client;
use crate::console::picker::{self, Choice, PickerMsg};
use crate::console::ui::{self, Kind, Tone};
use crate::theme::{
    SANS_MEDIUM, SANS_SEMIBOLD, TYPE_ITEM, TYPE_META, TYPE_SECTION, TYPE_UI, palette, skin,
};

const STEPS: [&str; 5] = ["Purpose", "Role", "Tools & limits", "Prompt", "Review"];
const PROPOSAL_STEPS: [&str; 3] = ["The proposal", "The role", "Decide"];
const EFFORTS: [&str; 6] = ["none", "low", "medium", "high", "xhigh", "max"];
const TOOL_GROUPS: &[(&str, &[&str])] = &[
    ("Code & files", &["code_engine", "tool_package"]),
    ("Plaza", &["plaza_hire"]),
    (
        "Weft",
        &["weft_status", "weft_diff", "weft_describe", "weft_log"],
    ),
];
const MIN_BUDGET: i64 = 200;
const MAX_BUDGET: i64 = 50_000;
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

/// A single-line field.
#[derive(Debug, Clone, Copy)]
pub enum Field {
    Id,
    Title,
    Budget,
}

/// A multi-line field.
#[derive(Debug, Clone, Copy)]
pub enum Area {
    Purpose,
    Collaboration,
    Prompt,
}

/// A list of text rows.
#[derive(Debug, Clone, Copy)]
pub enum ListKey {
    Responsibilities,
    Deliverables,
    Boundaries,
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
    /// The custom role whose delete is waiting for the owner's confirm.
    deleting_role: Option<String>,
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
    prompt: Content,
    /// The orchestrator's proposal under review; the last step approves or rejects it instead of spawning.
    proposal: Option<ProposalView>,
    rejecting: bool,
    /// The rejection reason, typed in a multi-line editor (Enter adds a line, as the web's textarea).
    reject_reason: Content,
    /// The project the spawned agents join at their first hello (a project's Team tab sets it); None: the console's
    /// project, if it is scoped to one, else they wait on the bench.
    pub project: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WizardMsg {
    Open,
    /// Opens the wizard for a project: the agents it spawns join that project.
    OpenFor(String),
    Close,
    /// Spawned (or failed): the Agents screen reloads.
    Done(Result<(), String>),
    Loaded(Result<Loaded, String>),
    Retry,
    Go(usize),
    SetMode(Mode),
    PickRole(String),
    /// Opens (Some) or closes (None) the delete confirm on a custom role.
    AskDeleteRole(Option<String>),
    DeleteRole,
    /// The role is deleted and the list reloaded, or the hub refused (a role in use) and said why.
    RoleDeleted(Result<Vec<RoleView>, String>),
    Picker(PickerMsg),
    /// The owner's favourites after a star in the picker.
    Favourited(Result<Vec<String>, String>),
    Field(Field, String),
    Area(Area, text_editor::Action),
    ListEdit(ListKey, usize, String),
    ListAdd(ListKey),
    ListRemove(ListKey, usize),
    ToggleTool(&'static str, bool),
    Count(u32),
    EditPrompt(bool),
    ResetPrompt,
    Spawn,
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

    fn role(&self) -> Option<&RoleView> {
        self.roles.iter().find(|r| r.id == self.role_id)
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

    /// Whether the current step's fields allow moving on.
    fn valid(&self, step: usize) -> bool {
        match step {
            0 => self.mode == Mode::New || !self.role_id.is_empty(),
            1 => {
                self.mode == Mode::Existing
                    || (id_ok(&self.id)
                        && !self.title.trim().is_empty()
                        && !self.purpose_text().is_empty()
                        && !nonempty(&self.responsibilities).is_empty()
                        && !nonempty(&self.deliverables).is_empty())
            }
            2 => {
                !self.model.is_empty()
                    && (self.mode == Mode::Existing
                        || self
                            .budget
                            .is_some_and(|b| (MIN_BUDGET..=MAX_BUDGET).contains(&b)))
            }
            3 => self.mode == Mode::Existing || !self.current_prompt().trim().is_empty(),
            _ => true,
        }
    }

    fn refresh_choices(&mut self) {
        self.choices = worker_choices(&self.models, &self.worker_policy, &self.model);
    }

    fn list_mut(&mut self, key: ListKey) -> &mut Vec<String> {
        match key {
            ListKey::Responsibilities => &mut self.responsibilities,
            ListKey::Deliverables => &mut self.deliverables,
            ListKey::Boundaries => &mut self.boundaries,
        }
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

    /// Saves the new role (if any), then starts the agents; as the web's spawn does.
    fn spawn(&mut self, ctx: &Ctx) -> Task<WizardMsg> {
        self.busy = true;
        self.error = None;
        let client = ctx.client.clone();
        let model = self.model_value();
        let count = self.count;
        let (role_id, new_role) = match self.mode {
            Mode::New => (self.id.clone(), Some(self.role_json(&model))),
            Mode::Existing => (self.role_id.clone(), None),
        };
        // The agents join this project at their first hello; with none they wait on the bench.
        let project = self.project.clone().or_else(|| ctx.project.clone());
        client::task(
            move || {
                if let Some(def) = new_role {
                    client.post::<Value>("/v1/roles", &def)?;
                }
                client.post::<Value>(
                    "/v1/agents",
                    &json!({ "role": role_id, "count": count, "model": model, "project": project }),
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
        WizardMsg::Open => {
            *wizard = Wizard::fresh();
            wizard.open = true;
            return load(ctx);
        }
        WizardMsg::OpenFor(project) => {
            *wizard = Wizard::fresh();
            wizard.open = true;
            wizard.project = Some(project);
            return load(ctx);
        }
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
            // A proposal's steps are read-only, so they always move (as the web's do).
            let count = step_names(wizard).len();
            if to < count
                && (to <= wizard.step || wizard.proposal.is_some() || wizard.valid(wizard.step))
            {
                wizard.step = to;
                wizard.editing_prompt = false;
                wizard.rejecting = false;
            }
        }
        WizardMsg::SetMode(mode) => {
            wizard.mode = mode;
            wizard.deleting_role = None;
        }
        WizardMsg::PickRole(id) => {
            let model = wizard
                .roles
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.def.model.clone());
            wizard.role_id = id;
            if let Some(model) = model {
                let (base, effort) = split_model(&model);
                if !base.is_empty() {
                    wizard.model = base;
                    wizard.effort = effort;
                    wizard.refresh_choices();
                }
            }
        }
        WizardMsg::AskDeleteRole(id) => wizard.deleting_role = id,
        WizardMsg::DeleteRole => {
            let Some(id) = wizard.deleting_role.clone().filter(|_| !wizard.busy) else {
                return Task::none();
            };
            wizard.busy = true;
            wizard.error = None;
            let client = ctx.client.clone();
            return client::task(
                move || {
                    client.delete(&format!("/v1/roles/{id}"))?;
                    client.get::<Vec<RoleView>>("/v1/roles")
                },
                WizardMsg::RoleDeleted,
            );
        }
        WizardMsg::RoleDeleted(result) => {
            wizard.busy = false;
            wizard.deleting_role = None;
            match result {
                Ok(roles) => {
                    wizard.roles = roles;
                    if !wizard.roles.iter().any(|r| r.id == wizard.role_id) {
                        wizard.role_id.clear();
                    }
                }
                Err(e) => wizard.error = Some(e),
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
        WizardMsg::Field(field, value) => match field {
            Field::Id => {
                wizard.id_touched = true;
                wizard.id = value
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                    .collect();
            }
            Field::Title => {
                wizard.title = value;
                if !wizard.id_touched {
                    wizard.id = slug(&wizard.title);
                }
            }
            Field::Budget => {
                wizard.budget = value.trim().parse::<i64>().ok();
                wizard.budget_text = value;
            }
        },
        WizardMsg::Area(area, action) => match area {
            Area::Purpose => wizard.purpose.perform(action),
            Area::Collaboration => wizard.collaboration.perform(action),
            Area::Prompt => {
                wizard.prompt.perform(action);
                wizard.prompt_edited = Some(wizard.prompt.text());
            }
        },
        WizardMsg::ListEdit(key, i, value) => {
            if let Some(item) = wizard.list_mut(key).get_mut(i) {
                *item = value;
            }
        }
        WizardMsg::ListAdd(key) => wizard.list_mut(key).push(String::new()),
        WizardMsg::ListRemove(key, i) => {
            let list = wizard.list_mut(key);
            if i < list.len() {
                list.remove(i);
            }
        }
        WizardMsg::ToggleTool(tool, on) => {
            wizard.tools.retain(|t| t.as_str() != tool);
            if on {
                wizard.tools.push(tool.to_string());
            }
        }
        WizardMsg::Count(n) => wizard.count = n.clamp(1, MAX_AGENTS),
        WizardMsg::EditPrompt(on) => {
            if on {
                wizard.prompt = Content::with_text(&wizard.current_prompt());
            }
            wizard.editing_prompt = on;
        }
        WizardMsg::ResetPrompt => {
            wizard.prompt_edited = None;
            wizard.editing_prompt = false;
        }
        WizardMsg::Spawn => {
            if !wizard.busy {
                return if wizard.proposal.is_some() {
                    wizard.approve(ctx)
                } else {
                    wizard.spawn(ctx)
                };
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
    let mut body = column![stepper(wizard, step_names(wizard)), body_host(wizard)].spacing(14);
    if let Some(e) = &wizard.error {
        body = body.push(ui::error_line(e));
    }
    ui::modal(
        if wizard.proposal.is_some() {
            "Review the orchestrator's proposal"
        } else {
            "Spawn agents"
        },
        body.into(),
        nav(wizard),
        WizardMsg::Close,
        560.0,
    )
}

/// The step names: the spawn wizard's five, or a proposal's three.
fn step_names(w: &Wizard) -> &'static [&'static str] {
    if w.proposal.is_some() {
        &PROPOSAL_STEPS
    } else {
        &STEPS
    }
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

/// The navigation in the foot, right-aligned: Back, then Next or the last step's action (the spawn, or a proposal's
/// approve and reject).
fn nav<'a>(w: &'a Wizard) -> Vec<Element<'a, WizardMsg>> {
    let last = w.step + 1 == step_names(w).len();
    let back = w.step.checked_sub(1).filter(|_| !w.busy).map(WizardMsg::Go);
    let mut out = vec![ui::btn("Back", None, Kind::Ghost, back)];
    if w.proposal.is_some() {
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
                (!w.busy).then_some(WizardMsg::Spawn),
            ));
        }
    } else if !last {
        let next = (w.loaded && !w.busy && w.valid(w.step)).then_some(WizardMsg::Go(w.step + 1));
        out.push(ui::btn("Next", None, Kind::Primary, next));
    } else {
        let count = plural(w.count, "agent");
        let label = match w.mode {
            Mode::New => format!("Create role and spawn {count}"),
            Mode::Existing => format!("Spawn {count}"),
        };
        out.push(ui::btn(
            &label,
            None,
            Kind::Primary,
            (!w.busy).then_some(WizardMsg::Spawn),
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
    if w.proposal.is_some() {
        return proposal_body(w);
    }
    let existing = w.mode == Mode::Existing;
    match w.step {
        0 => purpose_step(w),
        1 if existing => role_preview(w),
        1 => role_form(w),
        2 if existing => existing_limits(w),
        2 => new_limits(w),
        3 if existing => existing_prompt(w),
        3 => prompt_step(w),
        _ => review_step(w),
    }
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

/// A single-line input (`.input`).
fn input<'a>(
    placeholder: &'a str,
    value: &'a str,
    on: impl Fn(String) -> WizardMsg + 'a,
) -> Element<'a, WizardMsg> {
    text_input(placeholder, value)
        .on_input(on)
        .padding([6, 10])
        .style(crate::theme::input)
        .into()
}

/// A multi-line editor (`.textarea`) of `height` pixels.
fn area<'a>(
    content: &'a Content,
    placeholder: &'a str,
    which: Area,
    height: f32,
) -> Element<'a, WizardMsg> {
    text_editor(content)
        .style(crate::theme::editor)
        .placeholder(placeholder)
        .height(Length::Fixed(height))
        .padding(8)
        .on_action(move |a| WizardMsg::Area(which, a))
        .into()
}

/// A choice card (`.ag-choice`): a bordered block on the sheet, the signal ring and ground when it is on.
fn choice_card<'a>(
    on: bool,
    content: Element<'a, WizardMsg>,
    msg: WizardMsg,
) -> Element<'a, WizardMsg> {
    button(container(content).padding(12).width(Length::Fill))
        .on_press(msg)
        .padding(0)
        .width(Length::Fill)
        .style(move |_: &Theme, status: button::Status| choice_style(on, status))
        .into()
}

fn choice_style(on: bool, status: button::Status) -> button::Style {
    let p = palette();
    let edge = if on {
        p.signal
    } else if matches!(status, button::Status::Hovered) {
        p.ink_3
    } else {
        p.rule
    };
    button::Style {
        background: Some(Background::Color(if on { p.signal_soft } else { p.sheet })),
        text_color: p.ink,
        border: Border::default()
            .width(1.0)
            .color(edge)
            .rounded(skin().radius),
        shadow: Shadow::default(),
        snap: true,
    }
}

// ---- The spawn wizard's steps -----------------------------------------------------------------------------------

/// Step 1: use an existing role (and pick it below) or define a new one; a custom role's delete confirm sits under
/// the list.
fn purpose_step<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let choice = |mode: Mode, name: &'static str, hint_text: &'static str| {
        choice_card(
            w.mode == mode,
            column![
                text(name)
                    .font(SANS_SEMIBOLD)
                    .size(TYPE_ITEM)
                    .color(palette().ink),
                muted(hint_text)
            ]
            .spacing(4)
            .into(),
            WizardMsg::SetMode(mode),
        )
    };
    let mut col = column![
        heading("What should this agent do?"),
        column![
            choice(
                Mode::Existing,
                "Use an existing role",
                "Start agents of a role the plaza already has."
            ),
            choice(
                Mode::New,
                "Define a new role",
                "Write the role properly first, then start agents of it."
            ),
        ]
        .spacing(12),
    ]
    .spacing(12);
    if w.mode == Mode::Existing {
        if w.roles.is_empty() {
            col = col.push(muted("The plaza has no roles yet. Define a new one."));
        } else {
            col = col.push(column(w.roles.iter().map(|r| role_card(w, r))).spacing(12));
        }
        if let Some(confirm) = confirm_row(w) {
            col = col.push(confirm);
        }
    }
    col.into()
}

/// A role as a choice (`.ag-choice`): its name, built-in or custom, its id, its purpose and its agents; a custom
/// role carries its own Delete role.
fn role_card<'a>(w: &'a Wizard, r: &RoleView) -> Element<'a, WizardMsg> {
    let source = if r.source == "builtin" {
        "built-in"
    } else {
        "custom"
    };
    let mut body = column![
        row![
            text(r.title.clone())
                .font(SANS_SEMIBOLD)
                .size(TYPE_ITEM)
                .color(palette().ink),
            ui::badge(source, Tone::Muted),
            ui::mono(r.id.clone(), TYPE_UI, palette().ink_3),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        muted(clip(&r.def.purpose, 180)),
        muted(format!("{} run this role", plural(r.agents, "agent"))),
    ]
    .spacing(4);
    if r.source != "builtin" {
        body = body.push(ui::btn_sm(
            "Delete role",
            None,
            Kind::Ghost,
            (!w.busy).then(|| WizardMsg::AskDeleteRole(Some(r.id.clone()))),
        ));
    }
    choice_card(
        w.role_id == r.id,
        body.into(),
        WizardMsg::PickRole(r.id.clone()),
    )
}

/// The delete confirm for the custom role the owner asked to delete (the web's inline confirm, a rule above it).
fn confirm_row<'a>(w: &'a Wizard) -> Option<Element<'a, WizardMsg>> {
    let id = w.deleting_role.as_deref()?;
    let r = w.roles.iter().find(|r| r.id == id)?;
    Some(
        column![
            ui::hairline(),
            column![
                ink_text(format!(
                    "Delete the role {} ({})? Its definition is removed for good.",
                    r.title, r.id
                )),
                row![
                    ui::btn_sm(
                        "Delete role",
                        None,
                        Kind::Danger,
                        (!w.busy).then_some(WizardMsg::DeleteRole)
                    ),
                    ui::btn_sm(
                        "Keep",
                        None,
                        Kind::Ghost,
                        Some(WizardMsg::AskDeleteRole(None))
                    ),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .padding([8, 0]),
        ]
        .into(),
    )
}

/// An existing role's read-only preview (step 2).
fn role_preview<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let Some(role) = w.role() else {
        return muted("No role selected. Go back and choose one.");
    };
    let d = &role.def;
    column![
        heading(&d.title),
        muted(format!("{}, an existing role", role.id)),
        ro("Purpose", &d.purpose),
        ro_list("Responsibilities", &d.responsibilities),
        ro_list("Deliverables", &d.deliverables),
        ro("Working with others", &d.collaboration),
        ro_list("Boundaries", &d.boundaries),
    ]
    .spacing(12)
    .into()
}

/// A new role's fields (step 2).
fn role_form<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    column![
        heading("Describe the role"),
        field(
            "Id",
            input("api-builder", &w.id, |s| WizardMsg::Field(Field::Id, s)),
            "Lowercase letters, digits and hyphens. It follows the title until you edit it by hand.",
        ),
        field(
            "Title",
            input("API builder", &w.title, |s| WizardMsg::Field(Field::Title, s)),
            "What this role is called, e.g. \"API builder\".",
        ),
        field(
            "Purpose",
            area(&w.purpose, "Builds and tests the plaza HTTP API.", Area::Purpose, 80.0),
            "Why the role exists, in a sentence or two. Good: \"Builds and tests the plaza HTTP API.\" Vague: \"helps with the project.\"",
        ),
        field(
            "Responsibilities",
            list_editor(ListKey::Responsibilities, &w.responsibilities, "Write and test the API server"),
            "Concrete, checkable duties: \"Write and test the API server\", not \"help with code\". At least one.",
        ),
        field(
            "Deliverables",
            list_editor(ListKey::Deliverables, &w.deliverables, "A merged change with tests"),
            "What the agent hands over when it is done. At least one.",
        ),
        field(
            "Working with others",
            area(&w.collaboration, "", Area::Collaboration, 80.0),
            "Who this role takes work from, and who reviews what it produces.",
        ),
        field(
            "Boundaries",
            list_editor(ListKey::Boundaries, &w.boundaries, "Does not merge to main"),
            "What it must not do. Optional, but a clear edge keeps roles from overlapping.",
        ),
    ]
    .spacing(12)
    .into()
}

/// An existing role's tools and budget, fixed by its definition, with the count and the model (step 3).
fn existing_limits<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let tools = w
        .role()
        .map(|r| r.def.tools.join(", "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Workspace tools only".into());
    let budget = w
        .role()
        .map(|r| r.def.budget.to_string())
        .unwrap_or_else(|| "—".into());
    column![
        heading("Tools & limits"),
        muted("This role's tools and budget are fixed by its definition; choose how many agents to start and on which model."),
        ro("Tools", &tools),
        ro("Budget", &budget),
        count_block(w),
        model_block(w),
    ]
    .spacing(12)
    .into()
}

/// A new role's extra tools, budget, count and model (step 3).
fn new_limits<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    column![
        heading("Tools & limits"),
        tools_block(w),
        field(
            "Task budget",
            input("2000", &w.budget_text, |s| WizardMsg::Field(
                Field::Budget,
                s
            )),
            "The most one task may spend: 200 to 50,000 (default 2,000).",
        ),
        count_block(w),
        model_block(w),
    ]
    .spacing(12)
    .into()
}

/// The extra tools, in their groups, as the web's fieldset.
fn tools_block<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let mut groups = column![].spacing(8);
    for &(group, names) in TOOL_GROUPS {
        let mut g = column![muted(group)].spacing(6);
        for &tool in names {
            let on = w.tools.iter().any(|t| t.as_str() == tool);
            g = g.push(
                checkbox(on)
                    .label(tool)
                    .on_toggle(move |v| WizardMsg::ToggleTool(tool, v)),
            );
        }
        groups = groups.push(g);
    }
    column![
        text("Extra tools")
            .font(SANS_MEDIUM)
            .size(12.5)
            .color(palette().ink_2),
        muted("Every agent already gets its workspace tools, the board, messages, memory and the toolbox; the plaza's laws decide the rest."),
        groups,
    ]
    .spacing(8)
    .into()
}

/// The agents' count, 1 to 8.
fn count_pick<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let options: Vec<u32> = (1..=MAX_AGENTS).collect();
    pick_list(options, Some(w.count), WizardMsg::Count)
        .width(Length::Fill)
        .style(crate::theme::pick_list)
        .into()
}

fn count_block<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    field(
        "Agents",
        count_pick(w),
        "How many agents of this role to start (1 to 8).",
    )
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

fn model_block<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    field(
        "Model and effort",
        model_pick(w, true),
        "Worker models only. Effort is saved with the model as model@effort; \"Model default\" leaves it to the model.",
    )
}

/// The prompt, generated from the fields or edited by hand (step 4).
fn prompt_step<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let edited = w.prompt_edited.is_some();
    let note = if w.editing_prompt {
        "Markdown. This text is saved as the role prompt."
    } else if edited {
        "Edited by hand."
    } else {
        "Generated from the fields in steps 2 and 3."
    };
    let mut bar = row![
        muted(note),
        ui::btn_sm(
            if w.editing_prompt { "Done" } else { "Edit" },
            None,
            Kind::Ghost,
            Some(WizardMsg::EditPrompt(!w.editing_prompt)),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if edited {
        bar = bar.push(ui::btn_sm(
            "Reset to generated",
            None,
            Kind::Ghost,
            Some(WizardMsg::ResetPrompt),
        ));
    }
    let body: Element<'a, WizardMsg> = if w.editing_prompt {
        text_editor(&w.prompt)
            .style(crate::theme::editor)
            .height(Length::Fixed(360.0))
            .padding(8)
            .on_action(|a| WizardMsg::Area(Area::Prompt, a))
            .into()
    } else {
        ui::markdown(&w.current_prompt())
    };
    column![heading("Prompt"), bar, body].spacing(12).into()
}

/// An existing role's prompt, read-only (step 4).
fn existing_prompt<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let prompt = match w.role() {
        Some(r) if !r.def.prompt.is_empty() => r.def.prompt.clone(),
        Some(r) => prompt_text(
            &r.def.title,
            &r.def.purpose,
            &r.def.responsibilities,
            &r.def.deliverables,
            &r.def.collaboration,
            &r.def.boundaries,
            r.def.budget,
        ),
        None => String::new(),
    };
    column![heading("Prompt"), ui::markdown(&prompt)]
        .spacing(12)
        .into()
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

/// The review's rows: the role, the agents, the model (step 5).
fn review_rows(w: &Wizard) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if w.mode == Mode::Existing {
        let title = w
            .role()
            .map(|r| r.def.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| w.role_id.clone());
        rows.push(("Role", format!("{title} ({}), existing", w.role_id)));
    } else {
        rows.push(("New role", format!("{} ({})", w.title.trim(), w.id)));
        rows.push(("Purpose", w.purpose_text()));
        rows.push(("Responsibilities", nonempty(&w.responsibilities).join("; ")));
        rows.push(("Deliverables", nonempty(&w.deliverables).join("; ")));
        rows.push(("Working with others", w.collaboration_text()));
        let boundaries = nonempty(&w.boundaries).join("; ");
        rows.push((
            "Boundaries",
            if boundaries.is_empty() {
                "—".into()
            } else {
                boundaries
            },
        ));
        rows.push((
            "Tools",
            if w.tools.is_empty() {
                "Workspace tools only".into()
            } else {
                w.tools.join(", ")
            },
        ));
        rows.push(("Budget", w.budget_value().to_string()));
    }
    rows.push(("Agents", plural(w.count, "agent")));
    let effort = if w.effort.is_empty() {
        String::new()
    } else {
        format!(" · {} effort", w.effort)
    };
    rows.push(("Model", format!("{}{effort}", model_label(w))));
    rows
}

/// The review (step 5): the rows as a definition list.
fn review_step<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let mut dl = column![].spacing(4);
    for (key, value) in review_rows(w) {
        dl = dl.push(
            row![
                ui::sans(key, TYPE_ITEM, palette().ink_3).width(Length::Fixed(150.0)),
                ui::sans(value, TYPE_ITEM, palette().ink).width(Length::Fill),
            ]
            .spacing(12),
        );
    }
    column![heading("Review and spawn"), dl].spacing(12).into()
}

/// A list of text rows that edit `items` in place, with `Add` and × to remove a row.
fn list_editor<'a>(
    key: ListKey,
    items: &'a [String],
    placeholder: &'a str,
) -> Element<'a, WizardMsg> {
    let mut col = column![].spacing(12);
    for (i, value) in items.iter().enumerate() {
        col = col.push(
            row![
                input(placeholder, value.as_str(), move |s| WizardMsg::ListEdit(
                    key, i, s
                )),
                ui::btn_sm(
                    "",
                    Some("x"),
                    Kind::Ghost,
                    Some(WizardMsg::ListRemove(key, i))
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    col.push(ui::btn_sm(
        "Add",
        None,
        Kind::Ghost,
        Some(WizardMsg::ListAdd(key)),
    ))
    .into()
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

fn clip(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= n {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(n).collect::<String>())
    }
}

fn nonempty(xs: &[String]) -> Vec<String> {
    xs.iter()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
