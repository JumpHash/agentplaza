//! A project's or a task's budget, as a panel the project's and the task's pages hold (the web's `budget` card): what it caps,
//! what is spent, held and left, the set-budget form, the ledger and, for a project, its tasks' budgets. The panel reloads
//! on the budget's own frames, on the session and card frames of its owner, and with the page's poll. The expert view
//! shows the micro-dollars and every ledger column. The numbers and words are in `budget_model.rs`.

use std::fmt;

use iced::widget::{column, container, pick_list, text_input};
use iced::{Element, Length, Task};

use super::Id;
use super::budget_model::{self as model, Form};
use super::work_ui::{self, Col, EDGE};
use crate::console::events;
use crate::console::work_api::{self, Budget, BudgetUnit, LedgerEntry, OwnerKind};
use crate::console::{Ctx, client, ui};
use crate::theme;

/// The newest ledger entries the panel shows.
const LEDGER_LIMIT: u32 = 50;

/// Why the unit cannot change once the budget has spend.
const UNIT_FIXED: &str =
    "Spend is counted in the unit it was set in, so the unit is fixed once there is spend.";

/// The panel: the owner whose budget it shows, the budget and its ledger as last read, and the set-budget form while it
/// is open.
#[derive(Debug, Default)]
pub struct Panel {
    owner: Option<(OwnerKind, String)>,
    budget: Option<Budget>,
    ledger: Vec<LedgerEntry>,
    /// Why the last read failed, or None; a failed reload keeps the figures already shown.
    failure: Option<String>,
    expert: bool,
    form: Option<Form>,
    /// A line for the last save, or why one was refused.
    note: Option<String>,
    saving: bool,
    /// A read is in flight; a frame that comes meanwhile only asks for one more.
    loading: bool,
    again: bool,
}

/// What a read gave: the budget, and its newest ledger entries.
#[derive(Debug, Clone)]
pub struct Read {
    budget: Budget,
    ledger: Vec<LedgerEntry>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen (a task's or a session's link).
    Go(Id),
    Loaded(Result<Read, String>),
    Expert(bool),
    OpenForm,
    CloseForm,
    FormUnit(UnitChoice),
    FormCap(String),
    FormWarn(String),
    FormMonthly(String),
    Save,
    Saved(Result<(), String>),
    /// A hub event: the budget, a session or a card of this owner reloads it.
    Event(events::Event),
}

/// A unit as the form's pick list offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitChoice(BudgetUnit);

impl fmt::Display for UnitChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(model::unit_name(self.0))
    }
}

impl Panel {
    /// The panel of a project's or a task's budget.
    pub fn new(kind: OwnerKind, id: &str) -> Panel {
        Panel {
            owner: Some((kind, id.to_string())),
            ..Default::default()
        }
    }

    /// Whether the panel shows the budget of this owner (so a page that opens another owner starts a panel of its own).
    pub fn owned_by(&self, kind: OwnerKind, id: &str) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|(k, i)| *k == kind && i == id)
    }
}

/// Reads the budget and its ledger (called when the page opens and on every poll tick).
pub fn load(panel: &mut Panel, ctx: &Ctx) -> Task<Msg> {
    let Some((kind, id)) = panel.owner.clone() else {
        return Task::none();
    };
    panel.loading = true;
    let client = ctx.client.clone();
    client::task(
        move || match kind {
            OwnerKind::Project => Ok(Read {
                budget: work_api::project_budget(&client, &id)?,
                ledger: work_api::project_ledger(&client, &id, LEDGER_LIMIT)?,
            }),
            OwnerKind::Task => Ok(Read {
                budget: work_api::task_budget(&client, &id)?,
                ledger: work_api::task_ledger(&client, &id, LEDGER_LIMIT)?,
            }),
        },
        Msg::Loaded,
    )
}

/// A read landed: one more runs if frames came in while it was in flight.
fn landed(panel: &mut Panel, ctx: &Ctx) -> Task<Msg> {
    panel.loading = false;
    if std::mem::take(&mut panel.again) {
        load(panel, ctx)
    } else {
        Task::none()
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn update(panel: &mut Panel, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(read)) => {
            panel.budget = Some(read.budget);
            panel.ledger = read.ledger;
            panel.failure = None;
            landed(panel, ctx)
        }
        Msg::Loaded(Err(message)) => {
            panel.failure = Some(message);
            landed(panel, ctx)
        }
        Msg::Expert(on) => {
            panel.expert = on;
            Task::none()
        }
        Msg::OpenForm => {
            panel.form = Some(model::form_of(panel.budget.as_ref()));
            panel.note = None;
            Task::none()
        }
        Msg::CloseForm => {
            panel.form = None;
            Task::none()
        }
        Msg::FormUnit(UnitChoice(unit)) => {
            // The caps were in the old unit, so they are cleared rather than converted.
            if let Some(form) = panel.form.as_mut() {
                form.unit = unit;
                form.cap.clear();
                form.monthly_cap.clear();
            }
            Task::none()
        }
        Msg::FormCap(text) => {
            if let Some(form) = panel.form.as_mut() {
                form.cap = text;
            }
            Task::none()
        }
        Msg::FormWarn(text) => {
            if let Some(form) = panel.form.as_mut() {
                form.warn_pct = text;
            }
            Task::none()
        }
        Msg::FormMonthly(text) => {
            if let Some(form) = panel.form.as_mut() {
                form.monthly_cap = text;
            }
            Task::none()
        }
        Msg::Save => save(panel, ctx),
        Msg::Saved(Ok(())) => {
            panel.saving = false;
            panel.form = None;
            panel.note = Some("Budget saved".into());
            load(panel, ctx)
        }
        Msg::Saved(Err(message)) => {
            panel.saving = false;
            panel.note = Some(message);
            Task::none()
        }
        Msg::Event(event) => {
            let Some((kind, id)) = &panel.owner else {
                return Task::none();
            };
            let relevant = match event.kind.as_str() {
                "budget" => model::is_budget_of(&event.data, *kind, id),
                "session" | "card" => model::names_owner(&event.data, *kind, id),
                _ => false,
            };
            if !relevant {
                return Task::none();
            }
            if panel.loading {
                panel.again = true;
                return Task::none();
            }
            load(panel, ctx)
        }
    }
}

/// Saves the form as the budget's new limits. A unit change that blanks a cap the budget has is refused, so the cap is
/// typed again; a refused form says why and stays open.
fn save(panel: &mut Panel, ctx: &Ctx) -> Task<Msg> {
    let (Some(form), Some((kind, id))) = (panel.form.clone(), panel.owner.clone()) else {
        return Task::none();
    };
    if let Some(budget) = &panel.budget {
        let drops_cap = model::drops_cap(budget.unit, budget.cap, form.unit, &form.cap);
        let drops_monthly = kind == OwnerKind::Project
            && model::drops_cap(
                budget.unit,
                budget.monthly_cap,
                form.unit,
                &form.monthly_cap,
            );
        if drops_cap || drops_monthly {
            panel.note = Some("Type the cap again in the new unit, or keep the unit.".into());
            return Task::none();
        }
    }
    let body = match model::budget_body(kind, &form) {
        Ok(body) => body,
        Err(problem) => {
            panel.note = Some(problem);
            return Task::none();
        }
    };
    panel.saving = true;
    let client = ctx.client.clone();
    client::task(
        move || match kind {
            OwnerKind::Project => work_api::set_project_budget(&client, &id, &body).map(|_| ()),
            OwnerKind::Task => work_api::set_task_budget(&client, &id, &body).map(|_| ()),
        },
        Msg::Saved,
    )
}

pub fn view<'a>(panel: &'a Panel) -> Element<'a, Msg> {
    let Some((kind, _)) = &panel.owner else {
        return column![].into();
    };
    let mut actions: Vec<Element<'a, Msg>> =
        vec![work_ui::switch("Expert", panel.expert, Msg::Expert)];
    if panel.budget.as_ref().is_some_and(|b| b.unit.is_some()) {
        actions.push(ui::btn(
            "Set budget",
            None,
            ui::Kind::Secondary,
            Some(Msg::OpenForm),
        ));
    }
    let card = ui::card_with("Budget", actions, body(panel, *kind));
    match &panel.form {
        Some(form) => ui::with_modal(card, Some(form_dialog(panel, form, *kind))),
        None => card,
    }
}

/// The card's body: the figures of a set budget, or the words of one that is not set; the project's tasks, the ledger.
fn body<'a>(panel: &'a Panel, kind: OwnerKind) -> Element<'a, Msg> {
    let Some(budget) = &panel.budget else {
        return match &panel.failure {
            Some(message) => ui::error_line(&format!("Could not load the budget: {message}")),
            None => ui::faint("Loading the budget…").into(),
        };
    };
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(message) = &panel.failure {
        lines.push(ui::error_line(&format!(
            "Could not refresh the budget: {message}"
        )));
    }
    if let Some(note) = &panel.note {
        lines.push(ui::faint(note.clone()).into());
    }
    match budget.unit {
        None => {
            lines.push(ui::sans("Not set.", 14.0, theme::palette().ink).into());
            lines.push(ui::btn(
                "Set budget",
                None,
                ui::Kind::Primary,
                Some(Msg::OpenForm),
            ));
        }
        Some(unit) => {
            if budget.paused {
                lines.push(ui::badge("Paused: at its cap", ui::Tone::Warn));
            }
            lines.push(figures(panel, budget, unit, kind));
        }
    }
    if kind == OwnerKind::Project && !budget.tasks.is_empty() {
        lines.push(ui::faint("Tasks").into());
        lines.push(tasks_table(&budget.tasks));
    }
    lines.push(ui::faint("Ledger").into());
    lines.push(ledger_table(panel, budget.unit.unwrap_or(BudgetUnit::Usd)));
    column(lines).spacing(12).width(Length::Fill).into()
}

/// The figures of a set budget: the cap, the spend, what is held and left, the warn share, and for a project the
/// reservation and the monthly cap.
fn figures<'a>(
    panel: &'a Panel,
    budget: &Budget,
    unit: BudgetUnit,
    kind: OwnerKind,
) -> Element<'a, Msg> {
    let figure = |value: Option<i64>| -> String {
        let text = model::cap_text(unit, value);
        match value {
            Some(micros) if panel.expert && unit == BudgetUnit::Usd => {
                format!("{text} · {} µ$", model::grouped(micros))
            }
            _ => text,
        }
    };
    let mut items: Vec<Element<'a, Msg>> = vec![
        work_ui::fact("Unit", ink(model::unit_name(unit).into())),
        work_ui::fact("Cap", ink(figure(budget.cap))),
        work_ui::fact("Spent", ink(figure(Some(budget.spent)))),
        work_ui::fact("In flight", ink(figure(Some(budget.held)))),
    ];
    if kind == OwnerKind::Project {
        items.push(work_ui::fact(
            "Reserved",
            ink(figure(Some(budget.reserved))),
        ));
    }
    items.push(work_ui::fact("Left", ink(figure(budget.left))));
    items.push(work_ui::fact(
        "Share spent",
        ink(model::share_text(
            budget.cap,
            budget.spent_pct,
            budget.warn_at_pct,
        )),
    ));
    if kind == OwnerKind::Project {
        let monthly = budget
            .monthly_cap
            .map_or_else(|| "None".to_string(), |cap| figure(Some(cap)));
        items.push(work_ui::fact("Monthly cap", ink(monthly)));
        let month = budget
            .month
            .map_or(String::new(), |m| format!(" in {}", model::month_text(m)));
        items.push(work_ui::fact(
            "This month",
            ink(format!("{}{month}", figure(Some(budget.month_spent)))),
        ));
    }
    if let Some((_, id)) = panel.owner.as_ref().filter(|_| panel.expert) {
        items.insert(
            0,
            work_ui::fact(
                "Owner",
                ui::mono(id.clone(), 13.0, theme::palette().ink).into(),
            ),
        );
    }
    work_ui::facts(items, 3)
}

/// A project's tasks that have a budget of their own: the task (a link), what it spent, and its cap in its own unit.
fn tasks_table<'a>(tasks: &'a [Budget]) -> Element<'a, Msg> {
    let cols = [Col::flex(4), Col::right(2), Col::right(2)];
    let rows: Vec<Element<'a, Msg>> = tasks
        .iter()
        .map(|t| {
            let spent = t
                .unit
                .map_or_else(|| "—".to_string(), |u| model::amount_text(u, t.spent));
            let cap = t
                .unit
                .map_or_else(|| "Not set".to_string(), |u| model::cap_text(u, t.cap));
            let line = work_ui::grid_row(
                &cols,
                vec![
                    ui::text_link(&t.owner, Msg::Go(Id::Task(t.owner.clone()))),
                    ink(spent),
                    ink(cap),
                ],
                EDGE,
            );
            container_line(line)
        })
        .collect();
    work_ui::table(&cols, &["Task", "Spent", "Cap"], rows, EDGE)
}

/// The ledger: the newest entries, in the budget's unit; the expert view adds the micro-dollars and the tokens.
fn ledger_table<'a>(panel: &'a Panel, unit: BudgetUnit) -> Element<'a, Msg> {
    if panel.ledger.is_empty() {
        return ui::faint("Nothing recorded yet.").into();
    }
    let mut cols = vec![
        Col::flex(2),
        Col::flex(3),
        Col::right(2),
        Col::flex(2),
        Col::flex(3),
    ];
    let mut labels = vec!["When", "What", "Amount", "Session", "Model"];
    if panel.expert {
        cols.extend([Col::right(2), Col::right(2)]);
        labels.extend(["Micro-dollars", "Tokens"]);
    }
    let rows: Vec<Element<'a, Msg>> = panel
        .ledger
        .iter()
        .map(|e| {
            let session = match &e.session {
                Some(id) => ui::text_link(
                    if panel.expert { id.as_str() } else { "Open" },
                    Msg::Go(Id::Session(id.clone())),
                ),
                None => ui::faint("—").into(),
            };
            let mut cells: Vec<Element<'a, Msg>> = vec![
                ui::faint(ui::ago(e.at_ms)).into(),
                ink(model::ledger_label(&e.kind)),
                ink(model::ledger_amount(unit, &e.kind, e.amount)),
                session,
                ink(e.model.clone().unwrap_or_else(|| "—".into())),
            ];
            if panel.expert {
                cells.push(ink(model::grouped(e.micros)));
                cells.push(ink(model::grouped(e.tokens)));
            }
            container_line(work_ui::grid_row(&cols, cells, EDGE))
        })
        .collect();
    work_ui::table(&cols, &labels, rows, EDGE)
}

/// The set-budget form in a sheet: the unit (fixed once there is spend), the cap (blank for none), and for a project the
/// warn share and the monthly cap.
fn form_dialog<'a>(panel: &'a Panel, form: &'a Form, kind: OwnerKind) -> Element<'a, Msg> {
    let locked = panel.budget.as_ref().is_some_and(model::unit_locked);
    let unit_field: Element<'a, Msg> = if locked {
        column![
            ink(model::unit_name(form.unit).into()),
            ui::faint(UNIT_FIXED)
        ]
        .spacing(4)
        .into()
    } else {
        let options: Vec<UnitChoice> = [BudgetUnit::Usd, BudgetUnit::Tokens]
            .map(UnitChoice)
            .to_vec();
        pick_list(options, Some(UnitChoice(form.unit)), Msg::FormUnit).into()
    };
    let cap_hint = match form.unit {
        BudgetUnit::Usd => "For example 25 or 2.50",
        BudgetUnit::Tokens => "For example 500000",
    };
    let mut fields: Vec<Element<'a, Msg>> = vec![
        field("Unit", unit_field),
        field(
            "Cap",
            text_input(cap_hint, &form.cap)
                .on_input(Msg::FormCap)
                .into(),
        ),
    ];
    if kind == OwnerKind::Project {
        fields.push(field(
            "Warn at, percent",
            text_input("80", &form.warn_pct)
                .on_input(Msg::FormWarn)
                .into(),
        ));
        fields.push(field(
            "Monthly cap",
            text_input(cap_hint, &form.monthly_cap)
                .on_input(Msg::FormMonthly)
                .into(),
        ));
    }
    let body = column(fields).spacing(12).width(Length::Fill);
    ui::modal(
        "Set budget",
        body.into(),
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseForm)),
            ui::btn(
                "Save",
                None,
                ui::Kind::Primary,
                (!panel.saving).then_some(Msg::Save),
            ),
        ],
        Msg::CloseForm,
        440.0,
    )
}

/// A form field: its label above the control, and a note beneath it when the field needs one.
fn field<'a>(label: &str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(label), control]
        .spacing(6)
        .width(Length::Fill)
        .into()
}

/// A line of the page's ink in the sans voice.
fn ink<'a>(words: String) -> Element<'a, Msg> {
    ui::sans(words, 14.0, theme::palette().ink).into()
}

/// A table row in a container that fills the width, so the table's rules run the whole card.
fn container_line<'a>(line: Element<'a, Msg>) -> Element<'a, Msg> {
    container(line).padding([9, 0]).width(Length::Fill).into()
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
