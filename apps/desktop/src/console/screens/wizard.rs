//! The new-agent wizard (`agent-wizard.ts`): define a role (or pick one), its model and effort,
//! its tools and its prompt, then spawn — a modal over the Agents screen.

use iced::widget::text_editor::Content;
use iced::widget::{checkbox, column, container, row, scrollable, text_editor, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Shadow, Task, Theme, Vector};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::console::client;
use crate::console::picker::{self, Choice, PickerMsg};
use crate::console::ui::{self, Kind};
use crate::console::Ctx;

const STEPS: [&str; 5] = ["Purpose", "Role", "Tools & limits", "Prompt", "Review"];
const EFFORTS: [&str; 6] = ["none", "low", "medium", "high", "xhigh", "max"];
const TOOL_GROUPS: &[(&str, &[&str])] = &[
    ("Code & files", &["code_engine", "tool_package"]),
    ("Plaza", &["plaza_hire"]),
    ("Git", &["plaza_git"]),
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
}

#[derive(Debug, Clone)]
pub enum WizardMsg {
    Open,
    Close,
    /// Spawned (or failed): the Agents screen reloads.
    Done(Result<(), String>),
    Loaded(Result<Loaded, String>),
    Retry,
    Go(usize),
    SetMode(Mode),
    PickRole(String),
    Picker(PickerMsg),
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
}

impl Wizard {
    fn fresh() -> Wizard {
        Wizard { budget_text: "2000".into(), budget: Some(2000), count: 1, ..Default::default() }
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
        self.prompt_edited.clone().unwrap_or_else(|| self.generated_prompt())
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
                    && (self.mode == Mode::Existing || self.budget.is_some_and(|b| (MIN_BUDGET..=MAX_BUDGET).contains(&b)))
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
        client::task(
            move || {
                if let Some(def) = new_role {
                    client.post::<Value>("/v1/roles", &def)?;
                }
                client.post::<Value>("/v1/agents", &json!({ "role": role_id, "count": count, "model": model }))?;
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
            Ok(Loaded { roles, settings, models })
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
            wizard.worker_policy = loaded.settings.model_policy.worker;
            wizard.models = loaded.models;
            if wizard.model.is_empty() {
                wizard.model = fallback;
            }
            wizard.refresh_choices();
            if wizard.model.is_empty() {
                if let Some(first) = wizard.choices.first() {
                    wizard.model = first.id.clone();
                }
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
            if to < STEPS.len() && (to <= wizard.step || wizard.valid(wizard.step)) {
                wizard.step = to;
                wizard.editing_prompt = false;
            }
        }
        WizardMsg::SetMode(mode) => wizard.mode = mode,
        WizardMsg::PickRole(id) => {
            let model = wizard.roles.iter().find(|r| r.id == id).map(|r| r.def.model.clone());
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
        WizardMsg::Picker(m) => {
            picker::update(&mut wizard.picker, &m);
            match m {
                PickerMsg::Pick(id) => wizard.model = split_model(&id).0,
                PickerMsg::Effort(effort) => wizard.effort = effort,
                _ => {}
            }
        }
        WizardMsg::Field(field, value) => match field {
            Field::Id => {
                wizard.id_touched = true;
                wizard.id = value.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-').collect();
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
                return wizard.spawn(ctx);
            }
        }
    }
    Task::none()
}

/// The modal (the caller stacks it over its screen while `open`).
pub fn view<'a>(wizard: &'a Wizard, ctx: &'a Ctx) -> Element<'a, WizardMsg> {
    let _ = ctx;
    let last = wizard.step + 1 == STEPS.len();
    let head = row![ui::mono("Spawn agents", 13.0, ui::INK), ui::btn("Close", None, Kind::Ghost, Some(WizardMsg::Close))]
        .spacing(12)
        .align_y(Alignment::Center);
    let steps = row(STEPS.iter().enumerate().map(|(i, name)| {
        let kind = if i == wizard.step { Kind::Secondary } else { Kind::Ghost };
        let go = if i < wizard.step { Some(WizardMsg::Go(i)) } else { None };
        ui::btn(&format!("{} {}", i + 1, name), None, kind, go)
    }))
    .spacing(4);
    let body: Element<'a, WizardMsg> = if wizard.loaded {
        body_for(wizard)
    } else {
        match &wizard.load_error {
            Some(e) => column![
                ui::error_line(&format!("Could not load roles and models: {e}")),
                ui::btn("Retry", None, Kind::Ghost, Some(WizardMsg::Retry)),
            ]
            .spacing(12)
            .into(),
            None => ui::loading(),
        }
    };
    let back = wizard.step.checked_sub(1).filter(|_| !wizard.busy).map(WizardMsg::Go);
    let next = (!last && wizard.loaded && !wizard.busy && wizard.valid(wizard.step)).then(|| WizardMsg::Go(wizard.step + 1));
    let count = plural(wizard.count, "agent");
    let forward = if last {
        let label = match wizard.mode {
            Mode::New => format!("Create role and spawn {count}"),
            Mode::Existing => format!("Spawn {count}"),
        };
        ui::btn(&label, None, Kind::Primary, (!wizard.busy).then_some(WizardMsg::Spawn))
    } else {
        ui::btn("Next", None, Kind::Primary, next)
    };
    let nav = row![ui::btn("Back", None, Kind::Ghost, back), forward].spacing(8);

    let mut card = column![head, steps, scrollable(body).height(Length::Fixed(440.0))].spacing(16);
    if let Some(e) = &wizard.error {
        card = card.push(ui::error_line(e));
    }
    let card = card.push(nav);
    let sheet = container(card).padding(24).width(Length::Fill).max_width(720.0).style(|_: &Theme| container::Style {
        background: Some(Background::Color(ui::SHEET)),
        border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
        shadow: Shadow { color: Color { a: 0.35, ..ui::INK }, offset: Vector::new(4.0, 4.0), blur_radius: 0.5 },
        ..Default::default()
    });
    container(sheet)
        .padding(24)
        .center(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(Color { a: 0.4, ..Color::BLACK })),
            ..Default::default()
        })
        .into()
}

fn body_for<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let existing = w.mode == Mode::Existing;
    let (heading, content) = match w.step {
        0 => ("What should this agent do?", purpose_step(w)),
        1 if existing => ("The role", role_preview(w)),
        1 => ("Describe the role", role_form(w)),
        2 if existing => ("Tools & limits", existing_limits(w)),
        2 => ("Tools & limits", new_limits(w)),
        3 if existing => ("Prompt", existing_prompt(w)),
        3 => ("Prompt", prompt_step(w)),
        _ => ("Review and spawn", review_step(w)),
    };
    column![ui::mono(heading, 14.0, ui::INK), content].spacing(14).into()
}

fn purpose_step<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let mut col = column![row![
        choice(w.mode == Mode::Existing, "Use an existing role", "Start agents of a role the plaza already has.", WizardMsg::SetMode(Mode::Existing)),
        choice(w.mode == Mode::New, "Define a new role", "Write the role properly first, then start agents of it.", WizardMsg::SetMode(Mode::New)),
    ]
    .spacing(12)]
    .spacing(14);
    if w.mode == Mode::Existing {
        if w.roles.is_empty() {
            col = col.push(ui::faint("The plaza has no roles yet. Define a new one."));
        }
        for r in &w.roles {
            let source = if r.source == "builtin" { "built-in" } else { "custom" };
            let kind = if w.role_id == r.id { Kind::Primary } else { Kind::Secondary };
            col = col.push(
                column![
                    ui::btn(&format!("{} · {}", r.title, r.id), None, kind, Some(WizardMsg::PickRole(r.id.clone()))),
                    ui::faint(format!("{source} · {} · {}", clip(&r.def.purpose, 180), plural(r.agents, "agent"))).size(11.0),
                ]
                .spacing(4),
            );
        }
    }
    col.into()
}

fn choice<'a>(on: bool, name: &str, hint: &str, msg: WizardMsg) -> Element<'a, WizardMsg> {
    let kind = if on { Kind::Primary } else { Kind::Secondary };
    column![ui::btn(name, None, kind, Some(msg)), ui::faint(hint.to_string()).size(11.0)]
        .spacing(6)
        .width(Length::FillPortion(1))
        .into()
}

fn role_preview<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let Some(role) = w.role() else {
        return ui::faint("No role selected. Go back and choose one.").into();
    };
    let d = &role.def;
    column![
        ui::mono(d.title.clone(), 13.0, ui::INK),
        ui::faint(format!("{}, an existing role", role.id)),
        read_only("Purpose", d.purpose.clone()),
        read_only("Responsibilities", list_text(&d.responsibilities)),
        read_only("Deliverables", list_text(&d.deliverables)),
        read_only("Working with others", d.collaboration.clone()),
        read_only("Boundaries", list_text(&d.boundaries)),
    ]
    .spacing(12)
    .into()
}

fn role_form<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    column![
        field(
            "Id",
            input("api-builder", &w.id, |s| WizardMsg::Field(Field::Id, s)),
            "Lowercase letters, digits and hyphens. It follows the title until you edit it by hand.",
        ),
        field("Title", input("API builder", &w.title, |s| WizardMsg::Field(Field::Title, s)), "What this role is called, e.g. \"API builder\"."),
        field(
            "Purpose",
            area(&w.purpose, "Builds and tests the plaza HTTP API.", Area::Purpose, 90.0),
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
            area(&w.collaboration, "", Area::Collaboration, 90.0),
            "Who this role takes work from, and who reviews what it produces.",
        ),
        field(
            "Boundaries",
            list_editor(ListKey::Boundaries, &w.boundaries, "Does not merge to main"),
            "What it must not do. Optional, but a clear edge keeps roles from overlapping.",
        ),
    ]
    .spacing(14)
    .into()
}

fn existing_limits<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let tools = w
        .role()
        .map(|r| r.def.tools.join(", "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Workspace tools only".into());
    let budget = w.role().map(|r| r.def.budget.to_string()).unwrap_or_else(|| "—".into());
    column![
        ui::faint("This role's tools and budget are fixed by its definition; choose how many agents to start and on which model."),
        read_only("Tools", tools),
        read_only("Budget", budget),
        count_block(w),
        model_block(w),
    ]
    .spacing(14)
    .into()
}

fn new_limits<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let mut tools = column![ui::label("Extra tools"), ui::faint("Every agent already gets its workspace tools, the board, messages, memory and the toolbox; the plaza's laws decide the rest.")].spacing(10);
    for &(group, names) in TOOL_GROUPS {
        let mut g = column![ui::label(group)].spacing(6);
        for &tool in names {
            let on = w.tools.iter().any(|t| t.as_str() == tool);
            g = g.push(checkbox(tool, on).on_toggle(move |v| WizardMsg::ToggleTool(tool, v)));
        }
        tools = tools.push(g);
    }
    column![
        tools,
        field(
            "Task budget",
            input("2000", &w.budget_text, |s| WizardMsg::Field(Field::Budget, s)),
            "The most one task may spend: 200 to 50,000 (default 2,000).",
        ),
        count_block(w),
        model_block(w),
    ]
    .spacing(14)
    .into()
}

fn count_block<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let stepper = row![
        ui::btn("−", None, Kind::Secondary, (w.count > 1).then_some(WizardMsg::Count(w.count.saturating_sub(1)))),
        ui::mono(w.count.to_string(), 13.0, ui::INK),
        ui::btn("+", None, Kind::Secondary, (w.count < MAX_AGENTS).then_some(WizardMsg::Count(w.count + 1))),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    field("Agents", stepper.into(), "How many agents of this role to start (1 to 8).")
}

fn model_block<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let picked = picker::view(&w.picker, &w.choices, &w.model, Some(w.effort.as_str()), WizardMsg::Picker);
    field(
        "Model and effort",
        picked,
        "Worker models only. Effort is saved with the model as model@effort; \"Model default\" leaves it to the model.",
    )
}

fn prompt_step<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let edited = w.prompt_edited.is_some();
    let note = if w.editing_prompt {
        "Markdown. This text is saved as the role prompt."
    } else if edited {
        "Edited by hand."
    } else {
        "Generated from the fields in steps 2 and 3."
    };
    let bar = row![
        ui::faint(note),
        ui::btn("Reset to generated", None, Kind::Ghost, edited.then_some(WizardMsg::ResetPrompt)),
        ui::btn(
            if w.editing_prompt { "Done" } else { "Edit" },
            None,
            Kind::Ghost,
            Some(WizardMsg::EditPrompt(!w.editing_prompt)),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let body: Element<'a, WizardMsg> = if w.editing_prompt {
        text_editor(&w.prompt)
            .height(Length::Fixed(360.0))
            .on_action(|a| WizardMsg::Area(Area::Prompt, a))
            .into()
    } else {
        ui::markdown(&w.current_prompt())
    };
    column![bar, body].spacing(10).into()
}

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
    ui::markdown(&prompt)
}

fn review_step<'a>(w: &'a Wizard) -> Element<'a, WizardMsg> {
    let mut rows: Vec<(String, String)> = Vec::new();
    if w.mode == Mode::Existing {
        let title = w.role().map(|r| r.def.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| w.role_id.clone());
        rows.push(("Role".into(), format!("{title} ({}), existing", w.role_id)));
    } else {
        rows.push(("New role".into(), format!("{} ({})", w.title.trim(), w.id)));
        rows.push(("Purpose".into(), w.purpose_text()));
        rows.push(("Responsibilities".into(), list_text(&nonempty(&w.responsibilities))));
        rows.push(("Deliverables".into(), list_text(&nonempty(&w.deliverables))));
        rows.push(("Working with others".into(), w.collaboration_text()));
        rows.push(("Boundaries".into(), list_text(&nonempty(&w.boundaries))));
        rows.push(("Tools".into(), if w.tools.is_empty() { "Workspace tools only".into() } else { w.tools.join(", ") }));
        rows.push(("Budget".into(), w.budget_value().to_string()));
    }
    rows.push(("Agents".into(), plural(w.count, "agent")));
    let label = w
        .choices
        .iter()
        .find(|c| c.id == w.model)
        .map(|c| c.label.clone())
        .unwrap_or_else(|| if w.model.is_empty() { "Model default".into() } else { w.model.clone() });
    let effort = if w.effort.is_empty() { String::new() } else { format!(" · {} effort", w.effort) };
    rows.push(("Model".into(), format!("{label}{effort}")));

    let mut col = column![].spacing(10);
    for (key, value) in rows {
        col = col.push(row![ui::label(&key).width(Length::Fixed(160.0)), ui::prose(value)].spacing(12));
    }
    col.into()
}

fn read_only<'a>(label: &str, body: String) -> Element<'a, WizardMsg> {
    let body = if body.trim().is_empty() { "—".to_string() } else { body };
    column![ui::label(label), ui::prose(body)].spacing(4).into()
}

/// A block of label, control and hint, as the console's forms lay them out.
fn field<'a>(label: &str, control: Element<'a, WizardMsg>, hint: &str) -> Element<'a, WizardMsg> {
    column![ui::label(label), control, ui::faint(hint.to_string()).size(11.0)].spacing(6).into()
}

fn input<'a>(placeholder: &'a str, value: &'a str, on: impl Fn(String) -> WizardMsg + 'a) -> Element<'a, WizardMsg> {
    text_input(placeholder, value).on_input(on).padding(8).into()
}

fn area<'a>(content: &'a Content, placeholder: &'a str, which: Area, height: f32) -> Element<'a, WizardMsg> {
    text_editor(content)
        .placeholder(placeholder)
        .height(Length::Fixed(height))
        .on_action(move |a| WizardMsg::Area(which, a))
        .into()
}

/// A list of text rows that edit `items` in place; `Add` and the × remove rows.
fn list_editor<'a>(key: ListKey, items: &'a [String], placeholder: &'a str) -> Element<'a, WizardMsg> {
    let mut col = column![].spacing(6);
    for (i, value) in items.iter().enumerate() {
        col = col.push(
            row![
                text_input(placeholder, value).on_input(move |s| WizardMsg::ListEdit(key, i, s)).padding(8),
                ui::btn("×", None, Kind::Ghost, Some(WizardMsg::ListRemove(key, i))),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }
    col.push(ui::btn("Add", None, Kind::Secondary, Some(WizardMsg::ListAdd(key)))).into()
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

fn list_text(xs: &[String]) -> String {
    if xs.is_empty() {
        "—".to_string()
    } else {
        xs.iter().map(|x| format!("• {x}")).collect::<Vec<_>>().join("\n")
    }
}

fn nonempty(xs: &[String]) -> Vec<String> {
    xs.iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
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
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Splits `provider/model[@effort]`; an effort the wizard does not know is dropped.
fn split_model(value: &str) -> (String, String) {
    match value.split_once('@') {
        Some((base, effort)) => (base.to_string(), if EFFORTS.contains(&effort) { effort.to_string() } else { String::new() }),
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
            .unwrap_or_else(|| Choice { id: base.clone(), provider: provider_of(&base), label: base.clone() });
        out.push(choice);
    }
    for c in all {
        if !out.iter().any(|o| o.id == c.id) {
            out.push(c);
        }
    }
    if !current.is_empty() && !out.iter().any(|c| c.id == current) {
        out.push(Choice { id: current.to_string(), provider: provider_of(current), label: current.to_string() });
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
    let bullets = |xs: &[String]| nonempty(xs).iter().map(|x| format!("- {x}")).collect::<Vec<_>>().join("\n");
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
