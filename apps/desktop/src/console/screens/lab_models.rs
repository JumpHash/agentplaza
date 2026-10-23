//! The Lab's model registry (`LabModels`): every registered model as a table or as cards, with its
//! state, size, parameters, source job and (once in shadow) its agreement rate. Its actions are
//! shadow (trained), promote (in shadow, confirmed: the owner's act), retire (confirmed) and import
//! (a directory a training job wrote). Rows and links open `LabModelDetail(id)`. The model detail
//! screen shares this file's badge, action row, dialogs and [`Controls`], so both screens agree on
//! what a state means and what an action asks. The web's `lab-models.ts` is the twin.

use iced::widget::{Space, column, row, text, text_input};
use iced::{Alignment, Element, Length, Subscription, Task};
use serde::Deserialize;

use super::Id;
use super::lab::{self, Section};
use super::lab_common;
use super::lab_hf;
use crate::console::Ctx;
use crate::console::client::{self, Client};
use crate::console::lab_api::{self, ModelView};
use crate::console::ui::{self, Kind, Tone};
use crate::theme::SANS;

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
                let Dialog::Import(path) = &self.dialog else {
                    return None;
                };
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
pub fn run<M: Send + 'static>(
    client: &Client,
    request: Request,
    done: impl Fn(Result<ModelView, String>) -> M + Send + 'static,
) -> Task<M> {
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
    m.source
        .get("servable_reason")
        .and_then(|v| v.as_str())
        .map(str::to_string)
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
        Some(rate) if m.shadow.runs > 0 => format!(
            "{} · {} of {}",
            lab_common::percent(rate),
            lab_common::count(m.shadow.agreed),
            lab_common::count(m.shadow.runs)
        ),
        _ => "—".to_string(),
    }
}

/// A model's actions as the registry allows them: Shadow while trained, Promote while in shadow,
/// Retire while not retired. Each opens its confirm (Promote, Retire) or runs at once (Shadow).
pub fn actions<'a, M: Clone + 'a>(
    m: &ModelView,
    busy: bool,
    act: impl Fn(Act) -> M + Copy + 'a,
) -> Element<'a, M> {
    let id = m.id.clone();
    let mut items: Vec<Element<'a, M>> = Vec::new();
    if m.state == "trained" {
        items.push(ui::btn_sm(
            "Shadow",
            Some("play"),
            Kind::Secondary,
            (!busy).then(|| act(Act::Shadow(id.clone()))),
        ));
    }
    if m.state == "shadow" {
        items.push(ui::btn_sm(
            "Promote",
            Some("check"),
            Kind::Primary,
            (!busy).then(|| act(Act::Ask(Dialog::Promote(id.clone())))),
        ));
    }
    if m.state != "retired" {
        items.push(ui::btn_sm(
            "Retire",
            Some("ban"),
            Kind::Danger,
            (!busy).then(|| act(Act::Ask(Dialog::Retire(id.clone())))),
        ));
    }
    row(items).spacing(8).align_y(Alignment::Center).into()
}

/// The open dialog (import, promote or retire) as a card above the page; None while closed.
pub fn dialog<'a, M: Clone + 'a>(
    c: &'a Controls,
    act: impl Fn(Act) -> M + Copy + 'a,
) -> Option<Element<'a, M>> {
    let busy = c.busy;
    match &c.dialog {
        Dialog::Closed => None,
        Dialog::Import(path) => {
            let field = text_input("/absolute/path/to/a/job-directory", path)
                .style(crate::theme::input)
                .on_input(move |p| act(Act::ImportPath(p)))
                .on_submit(act(Act::Import))
                .padding(8);
            let body = column![
                ui::faint(
                    "An absolute path to a directory holding model.onnx and model_card.json."
                ),
                field,
                footer(
                    c,
                    act,
                    ui::btn(
                        "Import",
                        Some("plus"),
                        Kind::Primary,
                        (!busy).then(|| act(Act::Import))
                    )
                ),
            ]
            .spacing(12);
            Some(ui::card("Import a model", body.into()))
        }
        Dialog::Promote(id) => {
            let body = column![
                ui::faint(format!("Promote {id} to a model agent.")),
                ui::faint("Promotion gives the model its own agent on the roster: a CID, no VM, no tools and no chat. Other agents can invoke it with invoke_model where its card allows their role. Nobody can assign it tasks or message it."),
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
fn footer<'a, M: Clone + 'a>(
    c: &'a Controls,
    act: impl Fn(Act) -> M + Copy + 'a,
    confirm: Element<'a, M>,
) -> Element<'a, M> {
    let mut lines: Vec<Element<'a, M>> = Vec::new();
    if let Some(problem) = c.problem.as_deref() {
        lines.push(ui::error_line(problem));
    }
    lines.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Cancel", None, Kind::Ghost, Some(act(Act::Dismiss))),
            confirm
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    column(lines).spacing(12).into()
}

/// A text value in the table and card voice: mono, ink-2.
pub fn value<'a, M: 'a>(s: String) -> Element<'a, M> {
    text(s)
        .font(SANS)
        .size(12.0)
        .color(crate::theme::palette().ink_2)
        .width(Length::Fill)
        .into()
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
        move || {
            if base {
                c.get::<ModelList>("/v1/foundry/models?kind=base")
                    .map(|l| l.models)
            } else {
                lab_api::list_models(&c)
            }
        },
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
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(dialog) = dialog(&state.controls, Msg::Act) {
        parts.push(dialog);
    } else if let Some(problem) = state.controls.problem.as_deref() {
        parts.push(lab_common::err(problem));
    }
    if let Some(err) = state.error.as_deref() {
        parts.push(lab_common::err(err));
    }
    let import = || {
        ui::btn_sm(
            "Import a model",
            Some("plus"),
            Kind::Primary,
            Some(Msg::Act(Act::ImportOpen)),
        )
    };
    let base = state.base_only;
    let tabs = ui::tabs(
        &[(false, "All models"), (true, "Base models")],
        base,
        Msg::Kind,
    );
    parts.push(tabs);
    match &state.models {
        None => parts.push(match state.error.as_deref() {
            Some(err) => lab_common::err(err),
            None => lab_common::note("Loading models"),
        }),
        Some(list) if list.is_empty() => parts.push(lab_common::empty(
            "cpu",
            if base {
                "No base models yet"
            } else {
                "No models yet"
            },
            None,
            Some(import()),
        )),
        Some(list) => {
            parts.push(toolbar(state, list));
            parts.push(match state.view {
                View::Table => table(list, state.controls.busy),
                View::Cards => cards(list, state.controls.busy),
            });
        }
    }
    parts.push(lab_hf::view(&state.hf).map(Msg::Hf));
    let body: Element<'a, Msg> = ui::scroll(column(parts).spacing(16).width(Length::Fill))
        .height(Length::Fill)
        .into();
    lab::frame(Section::Models, body, Msg::Go)
}

/// The count line, the Table | Cards switch and the Import action, as the web's `.lab2-row`:
/// `3 models · 0 in shadow · 1 promoted`.
fn toolbar<'a>(state: &'a State, list: &[ModelView]) -> Element<'a, Msg> {
    let shadow = list.iter().filter(|m| m.state == "shadow").count() as u64;
    let promoted = list.iter().filter(|m| m.state == "promoted").count() as u64;
    let n = list.len() as u64;
    let noun = if n == 1 { "model" } else { "models" };
    let summary = format!(
        "{} {noun} · {} in shadow · {} promoted",
        lab_common::count(n),
        lab_common::count(shadow),
        lab_common::count(promoted)
    );
    row![
        lab_common::note(summary),
        Space::new().width(Length::Fill),
        ui::seg(
            &[(View::Table, "Table"), (View::Cards, "Cards")],
            state.view,
            Msg::View
        ),
        ui::btn(
            "Import",
            Some("plus"),
            Kind::Ghost,
            Some(Msg::Act(Act::ImportOpen)),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .wrap()
    .into()
}

/// The registry as the web's table: name and id (linked), version, state, size, parameters, created,
/// source job, agreement and the actions.
fn table<'a>(list: &'a [ModelView], busy: bool) -> Element<'a, Msg> {
    let cols = [
        lab_common::col("Model", 5, false),
        lab_common::col("Version", 2, false),
        lab_common::col("State", 2, false),
        lab_common::col("Size", 2, true),
        lab_common::col("Params", 2, true),
        lab_common::col("Created", 3, false),
        lab_common::col("Source job", 3, false),
        lab_common::col("Agreement", 3, true),
        lab_common::col("", 5, false),
    ];
    let rows: Vec<lab_common::Row<'a, Msg>> = list
        .iter()
        .map(|m| {
            let name = column![
                ui::link(
                    &m.card.name,
                    Msg::Go(Id::LabModelDetail(m.id.clone())),
                    false
                ),
                ui::mono(m.id.clone(), 11.0, crate::theme::palette().ink_3),
            ]
            .spacing(2);
            let job: Element<'a, Msg> = match &m.source_job {
                Some(job) => ui::link(job, Msg::Go(Id::LabJobDetail(job.clone())), true),
                None => value("—".to_string()),
            };
            lab_common::Row {
                cells: vec![
                    name.into(),
                    value(m.card.version.clone()),
                    state_cell(m),
                    value(lab_common::size(m.size_bytes)),
                    value(lab_common::count(m.parameters)),
                    value(lab_common::time(m.created_ms)),
                    job,
                    value(agreement_text(m)),
                    actions(m, busy, Msg::Act),
                ],
                on_press: None,
            }
        })
        .collect();
    lab_common::table(&cols, rows)
}

/// The registry as cards, three to a row (the web's `lab2-grid`).
fn cards<'a>(list: &'a [ModelView], busy: bool) -> Element<'a, Msg> {
    let mut lines = column(Vec::<Element<'a, Msg>>::new()).spacing(12);
    for chunk in list.chunks(CARDS) {
        let mut line: Vec<Element<'a, Msg>> = chunk.iter().map(|m| model_card(m, busy)).collect();
        while line.len() < CARDS {
            line.push(Space::new().width(Length::FillPortion(1)).into());
        }
        lines = lines.push(row(line).spacing(12));
    }
    lines.into()
}

/// One model as the web's card: its state and id with version, the reason it is not servable yet,
/// the headline figures, and the actions (Open first).
fn model_card<'a>(m: &'a ModelView, busy: bool) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = vec![
        row![
            ui::badge(&m.state, tone(&m.state)),
            lab_common::note(format!("{} · v{}", m.id, m.card.version)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    ];
    if m.state == "imported" {
        body.push(lab_common::note(
            reason_of(m).unwrap_or_else(|| "Not servable yet.".to_string()),
        ));
    }
    body.push(
        column![
            value(format!("Size {}", lab_common::size(m.size_bytes))),
            value(format!("Params {}", lab_common::count(m.parameters))),
            value(format!("Agreement {}", agreement_text(m))),
        ]
        .spacing(4)
        .into(),
    );
    let buttons: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm(
            "Open",
            None,
            Kind::Ghost,
            Some(Msg::Go(Id::LabModelDetail(m.id.clone()))),
        ),
        actions(m, busy, Msg::Act),
    ];
    ui::card_with(
        &m.card.name,
        buttons,
        column(body).spacing(10).width(Length::Fill).into(),
    )
}

/// A state's cell: the badge, and for an imported model the reason it is not servable yet.
fn state_cell<'a>(m: &ModelView) -> Element<'a, Msg> {
    let badge = ui::badge(&m.state, tone(&m.state));
    if m.state == "imported" {
        column![
            badge,
            ui::faint(reason_of(m).unwrap_or_else(|| "Not servable yet.".to_string()))
        ]
        .spacing(4)
        .into()
    } else {
        badge
    }
}

/// Cards per row in the models grid, as the web's `lab2-grid` at the console's width.
const CARDS: usize = 3;
