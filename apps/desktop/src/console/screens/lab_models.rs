//! The Lab's model registry (`LabModels`): every registered model as a table or as cards, with its
//! state, size, parameters, source job and (once in shadow) its agreement rate. Its actions are
//! shadow (trained), promote (in shadow, confirmed: the owner's act), retire (confirmed) and import
//! (a directory a training job wrote). Rows and links open `LabModelDetail(id)`. The model detail
//! screen shares this file's badge, action row, dialogs and [`Controls`], so both screens agree on
//! what a state means and what an action asks. The web's `lab-models.ts` is the twin.

use iced::widget::{column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length, Subscription, Task};
use serde::Deserialize;

use super::lab::{self, Section};
use super::lab_common;
use super::lab_hf;
use super::Id;
use crate::console::client::{self, Client};
use crate::console::lab_api::{self, ModelView};
use crate::console::ui::{self, Kind, Tone};
use crate::console::Ctx;
use crate::theme::{MONO, SANS};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Table,
    Cards,
}

/// The dialog open over the screen: the import form (its path), or a confirm for one model's id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Dialog {
    #[default]
    Closed,
    Import(String),
    Promote(String),
    Retire(String),
}

/// What a model's controls ask for. Each screen maps these to its own messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Shadow(String),
    Ask(Dialog),
    Dismiss,
    Confirm,
    ImportOpen,
    ImportPath(String),
    Import,
}

/// A call to apid that a control started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Shadow(String),
    Promote(String),
    Retire(String),
    Import(String),
}

/// The dialog, the last refusal and whether a call is in flight. Shared by the registry and the
/// model detail, which act on the same models the same way.
#[derive(Debug, Default)]
pub struct Controls {
    pub dialog: Dialog,
    /// The last refusal: shown in the open dialog, or above the page when none is open.
    pub problem: Option<String>,
    pub busy: bool,
}

impl Controls {
    /// Applies a control's act; returns the request it starts, if any (one call at a time).
    pub fn apply(&mut self, act: Act) -> Option<Request> {
        match act {
            Act::Shadow(id) => self.start(Request::Shadow(id)),
            Act::Ask(dialog) => {
                self.dialog = dialog;
                self.problem = None;
                None
            }
            Act::Dismiss => {
                self.dialog = Dialog::Closed;
                self.problem = None;
                None
            }
            Act::ImportOpen => {
                self.dialog = Dialog::Import(String::new());
                self.problem = None;
                None
            }
            Act::ImportPath(path) => {
                if let Dialog::Import(current) = &mut self.dialog {
                    *current = path;
                }
                None
            }
            Act::Confirm => match self.dialog.clone() {
                Dialog::Promote(id) => self.start(Request::Promote(id)),
                Dialog::Retire(id) => self.start(Request::Retire(id)),
                _ => None,
            },
            Act::Import => {
                let Dialog::Import(path) = &self.dialog else { return None };
                let path = path.trim().to_string();
                if path.is_empty() {
                    self.problem = Some("Give the directory path.".to_string());
                    return None;
                }
                self.start(Request::Import(path))
            }
        }
    }

    fn start(&mut self, request: Request) -> Option<Request> {
        if self.busy {
            return None;
        }
        self.busy = true;
        self.problem = None;
        Some(request)
    }

    /// A request's answer. A success closes the dialog and asks for a reload (true); a refusal
    /// keeps the dialog open with its message.
    pub fn finish(&mut self, result: Result<ModelView, String>) -> bool {
        self.busy = false;
        match result {
            Ok(_) => {
                self.dialog = Dialog::Closed;
                self.problem = None;
                true
            }
            Err(err) => {
                self.problem = Some(err);
                false
            }
        }
    }
}

/// Runs a request against apid as a task; its answer goes to `done`.
pub fn run<M: Send + 'static>(client: &Client, request: Request, done: impl Fn(Result<ModelView, String>) -> M + Send + 'static) -> Task<M> {
    let c = client.clone();
    client::task(
        move || match request {
            Request::Shadow(id) => lab_api::shadow_model(&c, &id),
            Request::Promote(id) => lab_api::promote_model(&c, &id),
            Request::Retire(id) => lab_api::retire_model(&c, &id),
            Request::Import(path) => lab_api::import_model(&c, &path),
        },
        done,
    )
}

/// A state's tone: trained blue, shadow signal (amber-orange), promoted green, retired grey.
pub fn tone(state: &str) -> Tone {
    match state {
        "trained" => Tone::Info,
        "shadow" => Tone::Signal,
        "promoted" => Tone::Ok,
        "imported" => Tone::Warn,
        _ => Tone::Muted,
    }
}

/// Why an imported model is not servable yet: the reason its source record keeps, if any.
pub fn reason_of(m: &ModelView) -> Option<String> {
    m.source.get("servable_reason").and_then(|v| v.as_str()).map(str::to_string)
}

/// Whether a model serves: an imported one does not (its reason says why), the others do.
pub fn servable_text(m: &ModelView) -> String {
    if m.state == "imported" {
        reason_of(m).unwrap_or_else(|| "no".to_string())
    } else {
        "yes".to_string()
    }
}

/// The agreement as a percent with its counts (`92% · 46 of 50`), or a dash before any shadow run.
pub fn agreement_text(m: &ModelView) -> String {
    match m.agreement_rate {
        Some(rate) if m.shadow.runs > 0 => format!("{} · {} of {}", lab_common::percent(rate), lab_common::count(m.shadow.agreed), lab_common::count(m.shadow.runs)),
        _ => "—".to_string(),
    }
}

/// A model's actions as the registry allows them: Shadow while trained, Promote while in shadow,
/// Retire while not retired. Each opens its confirm (Promote, Retire) or runs at once (Shadow).
pub fn actions<'a, M: Clone + 'a>(m: &ModelView, busy: bool, act: impl Fn(Act) -> M + Copy + 'a) -> Element<'a, M> {
    let id = m.id.clone();
    let mut items: Vec<Element<'a, M>> = Vec::new();
    if m.state == "trained" {
        items.push(ui::btn_sm("Shadow", Some("play"), Kind::Secondary, (!busy).then(|| act(Act::Shadow(id.clone())))));
    }
    if m.state == "shadow" {
        items.push(ui::btn_sm("Promote", Some("check"), Kind::Primary, (!busy).then(|| act(Act::Ask(Dialog::Promote(id.clone()))))));
    }
    if m.state != "retired" {
        items.push(ui::btn_sm("Retire", Some("ban"), Kind::Danger, (!busy).then(|| act(Act::Ask(Dialog::Retire(id.clone()))))));
    }
    row(items).spacing(8).align_y(Alignment::Center).into()
}

/// The open dialog (import, promote or retire) as a card above the page; None while closed.
pub fn dialog<'a, M: Clone + 'a>(c: &'a Controls, act: impl Fn(Act) -> M + Copy + 'a) -> Option<Element<'a, M>> {
    let busy = c.busy;
    match &c.dialog {
        Dialog::Closed => None,
        Dialog::Import(path) => {
            let field = text_input("/absolute/path/to/a/job-directory", path)
                .on_input(move |p| act(Act::ImportPath(p)))
                .on_submit(act(Act::Import))
                .padding(8);
            let body = column![
                ui::faint("An absolute path to a directory holding model.onnx and model_card.json."),
                field,
                footer(c, act, ui::btn("Import", Some("plus"), Kind::Primary, (!busy).then(|| act(Act::Import)))),
            ]
            .spacing(12);
            Some(ui::card("Import a model", body.into()))
        }
        Dialog::Promote(id) => {
            let body = column![
                ui::faint(format!("Promote {id} to a model agent.")),
                ui::faint("Promotion gives the model its own agent on the roster: a CID, no VM, no wallet, no tools and no chat. Other agents can invoke it with invoke_model where its card allows their role. Nobody can assign it tasks or message it."),
                footer(c, act, ui::btn("Promote", Some("check"), Kind::Primary, (!busy).then(|| act(Act::Confirm)))),
            ]
            .spacing(12);
            Some(ui::card("Promote model", body.into()))
        }
        Dialog::Retire(id) => {
            let body = column![
                ui::faint(format!("Retire {id}?")),
                ui::faint("No agent can invoke it after this. A promoted model's agent leaves the roster. The record and its shadow totals stay."),
                footer(c, act, ui::btn("Retire", Some("ban"), Kind::Danger, (!busy).then(|| act(Act::Confirm)))),
            ]
            .spacing(12);
            Some(ui::card("Retire model", body.into()))
        }
    }
}

/// A dialog's foot: the refusal (if any), then Cancel and the confirm.
fn footer<'a, M: Clone + 'a>(c: &'a Controls, act: impl Fn(Act) -> M + Copy + 'a, confirm: Element<'a, M>) -> Element<'a, M> {
    let mut lines: Vec<Element<'a, M>> = Vec::new();
    if let Some(problem) = c.problem.as_deref() {
        lines.push(ui::error_line(problem));
    }
    lines.push(
        row![Space::new().width(Length::Fill), ui::btn("Cancel", None, Kind::Ghost, Some(act(Act::Dismiss))), confirm]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
    );
    column(lines).spacing(12).into()
}

/// A text value in the table and card voice: mono, ink-2.
pub fn value<'a, M: 'a>(s: String) -> Element<'a, M> {
    text(s).font(SANS).size(12.0).color(crate::theme::palette().ink_2).width(Length::Fill).into()
}

#[derive(Debug, Default)]
pub struct State {
    /// The registry, None until the first load.
    pub models: Option<Vec<ModelView>>,
    pub error: Option<String>,
    pub view: View,
    pub controls: Controls,
    pub hf: lab_hf::State,
    /// Only the base models (`?kind=base`: the records imported as a fine-tune base).
    pub base_only: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ModelList {
    models: Vec<ModelView>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// A control's act (shadow, the dialogs, the import form).
    Act(Act),
    Loaded(Result<Vec<ModelView>, String>),
    /// A started request's answer.
    Done(Result<ModelView, String>),
    View(View),
    /// All models (false) or only the base models (true).
    Kind(bool),
    /// The Hugging Face panel's messages.
    Hf(lab_hf::Msg),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Hf(m) => lab_hf::go(m),
        _ => None,
    }
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let base = state.base_only;
    let list = client::task(
        move || if base { c.get::<ModelList>("/v1/foundry/models?kind=base").map(|l| l.models) } else { lab_api::list_models(&c) },
        Msg::Loaded,
    );
    Task::batch([list, lab_hf::load(&mut state.hf, ctx).map(Msg::Hf)])
}

/// The Hugging Face panel's live `foundry` frames, as this screen's messages.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    lab_hf::subscription(ctx).map(Msg::Hf)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(models)) => {
            state.models = Some(models);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(err)) => {
            state.error = Some(err);
            Task::none()
        }
        Msg::View(view) => {
            state.view = view;
            Task::none()
        }
        Msg::Kind(base) => {
            state.base_only = base;
            state.models = None;
            load(state, ctx)
        }
        Msg::Hf(m) => {
            let task = lab_hf::update(&mut state.hf, m, ctx).map(Msg::Hf);
            if std::mem::take(&mut state.hf.models_changed) {
                Task::batch([task, load(state, ctx)])
            } else {
                task
            }
        }
        Msg::Act(act) => match state.controls.apply(act) {
            Some(request) => run(&ctx.client, request, Msg::Done),
            None => Task::none(),
        },
        Msg::Done(result) => {
            if state.controls.finish(result) {
                load(state, ctx)
            } else {
                Task::none()
            }
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let import = ui::btn("Import", Some("plus"), Kind::Secondary, Some(Msg::Act(Act::ImportOpen)));
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::view_head("Models", vec![import])];
    if let Some(dialog) = dialog(&state.controls, Msg::Act) {
        parts.push(dialog);
    } else if let Some(problem) = state.controls.problem.as_deref() {
        parts.push(ui::error_line(problem));
    }
    if let Some(err) = state.error.as_deref() {
        parts.push(ui::error_line(err));
    }
    match &state.models {
        None => parts.push(match state.error.as_deref() {
            Some(err) => ui::empty("alert", "Models unreachable", err),
            None => ui::loading(),
        }),
        Some(list) if list.is_empty() && !state.base_only => parts.push(
            column![
                lab_common::empty(
                    "No models yet.",
                    "A training job that finishes writes model.onnx and model_card.json; import that directory here, or wait for a breeder's job to finish.",
                ),
                ui::btn("Import a model", Some("plus"), Kind::Primary, Some(Msg::Act(Act::ImportOpen))),
            ]
            .spacing(12)
            .align_x(Alignment::Center)
            .into(),
        ),
        Some(list) => {
            parts.push(toolbar(state, list));
            parts.push(if list.is_empty() {
                ui::faint("No base models yet. Import one from Hugging Face as base.").into()
            } else {
                match state.view {
                    View::Table => table(list, state.controls.busy),
                    View::Cards => cards(list, state.controls.busy),
                }
            });
        }
    }
    parts.push(lab_hf::view(&state.hf).map(Msg::Hf));
    lab::frame(Section::Models, column(parts).spacing(22).into(), Msg::Go)
}


/// The view switch and the count line: `3 models · 0 in shadow · 1 promoted`.
fn toolbar<'a>(state: &'a State, list: &[ModelView]) -> Element<'a, Msg> {
    let shadow = list.iter().filter(|m| m.state == "shadow").count() as u64;
    let promoted = list.iter().filter(|m| m.state == "promoted").count() as u64;
    let n = list.len() as u64;
    let noun = if n == 1 { "model" } else { "models" };
    let summary = format!("{} {noun} · {} in shadow · {} promoted", lab_common::count(n), lab_common::count(shadow), lab_common::count(promoted));
    row![
        ui::seg(&[(View::Table, "Table"), (View::Cards, "Cards")], state.view, Msg::View),
        ui::seg(&[(false, "All models"), (true, "Base models")], state.base_only, Msg::Kind),
        ui::faint(summary),
    ]
    .spacing(16)
    .align_y(Alignment::Center)
    .into()
}

/// One table cell: its share of the row, numbers right-aligned.
fn cell<'a>(portion: u16, right: bool, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    let c = container(content).width(Length::FillPortion(portion));
    if right {
        c.align_x(iced::alignment::Horizontal::Right).into()
    } else {
        c.into()
    }
}

fn head_cell<'a>(portion: u16, right: bool, label: &str) -> Element<'a, Msg> {
    cell(portion, right, text(label.to_string()).font(SANS).size(11.0).color(crate::theme::palette().ink_3))
}

/// The registry as a table: name and id (linked), version, state, size, parameters, created,
/// source job, agreement and the actions.
fn table<'a>(list: &'a [ModelView], busy: bool) -> Element<'a, Msg> {
    let head = row![
        head_cell(5, false, "Model"),
        head_cell(2, false, "Version"),
        head_cell(2, false, "State"),
        head_cell(2, true, "Size"),
        head_cell(2, true, "Params"),
        head_cell(3, false, "Created"),
        head_cell(3, false, "Source job"),
        head_cell(3, true, "Agreement"),
        head_cell(5, false, "Actions"),
    ]
    .spacing(10);
    let mut lines = column![head, ui::hairline()].spacing(8);
    for m in list {
        lines = lines.push(table_row(m, busy));
        lines = lines.push(ui::hairline());
    }
    ui::scroll(lines).width(Length::Fill).into()
}

fn table_row<'a>(m: &'a ModelView, busy: bool) -> Element<'a, Msg> {
    let name = column![
        ui::link(&m.card.name, Msg::Go(Id::LabModelDetail(m.id.clone())), true),
        text(m.id.clone()).font(MONO).size(11.0).color(crate::theme::palette().ink_3),
    ]
    .spacing(2);
    let job: Element<'a, Msg> = match &m.source_job {
        Some(job) => ui::link(job, Msg::Go(Id::LabJobDetail(job.clone())), true),
        None => value("—".to_string()),
    };
    row![
        cell(5, false, name),
        cell(2, false, value(m.card.version.clone())),
        cell(2, false, state_cell(m)),
        cell(2, true, value(lab_common::size(m.size_bytes))),
        cell(2, true, value(lab_common::count(m.parameters))),
        cell(3, false, value(lab_common::time(m.created_ms))),
        cell(3, false, job),
        cell(3, true, value(agreement_text(m))),
        cell(5, false, actions(m, busy, Msg::Act)),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// The registry as cards, two to a row: name, state, the headline figures and the actions.
fn cards<'a>(list: &'a [ModelView], busy: bool) -> Element<'a, Msg> {
    let mut lines = column(Vec::<Element<'a, Msg>>::new()).spacing(12);
    for pair in list.chunks(2) {
        let mut line: Vec<Element<'a, Msg>> = pair.iter().map(|m| model_card(m, busy)).collect();
        if pair.len() == 1 {
            line.push(Space::new().width(Length::FillPortion(1)).into());
        }
        lines = lines.push(row(line).spacing(12));
    }
    lines.into()
}

fn model_card<'a>(m: &'a ModelView, busy: bool) -> Element<'a, Msg> {
    let body = column![
        row![ui::badge(&m.state, tone(&m.state)), value(format!("{} · v{}", m.id, m.card.version))]
            .spacing(12)
            .align_y(Alignment::Center),
        field("Servable", value(servable_text(m))),
        field("Size", value(lab_common::size(m.size_bytes))),
        field("Params", value(lab_common::count(m.parameters))),
        field("Created", value(lab_common::time(m.created_ms))),
        field("Source job", match &m.source_job {
            Some(job) => ui::link(job, Msg::Go(Id::LabJobDetail(job.clone())), true),
            None => value("—".to_string()),
        }),
        field("Agreement", value(agreement_text(m))),
        actions(m, busy, Msg::Act),
    ]
    .spacing(10);
    container(ui::card(&m.card.name, body.into())).width(Length::FillPortion(1)).into()
}

/// A state's cell: the badge, and for an imported model the reason it is not servable yet.
fn state_cell<'a>(m: &ModelView) -> Element<'a, Msg> {
    let badge = ui::badge(&m.state, tone(&m.state));
    if m.state == "imported" {
        column![badge, ui::faint(reason_of(m).unwrap_or_else(|| "Not servable yet.".to_string()))].spacing(4).into()
    } else {
        badge
    }
}

/// A labelled figure in a card: the label in caps, the value beside it.
fn field<'a>(label: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![container(ui::label(label)).width(Length::Fixed(110.0)), value].spacing(12).align_y(Alignment::Center).into()
}
