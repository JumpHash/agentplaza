//! The Lab's recipes (`LabRecipes`), parity with the web's `#/lab/recipes`: each registered recipe
//! with its pin, its vet and its schema, in a grid of cards. Vet asks for a commit (checked the way
//! apid checks it), confirms, then records the vet; a refusal stays on the dialog. The list reloads
//! on every poll tick (the console's 4 s `Tick` calls `load`).

use iced::widget::{Space, column, container, row, stack, text_input};
use iced::{Alignment, Element, Length, Task};
use serde_json::Value;

use super::Id;
use super::lab::{self, Section};
use super::lab_common;
use crate::console::Ctx;
use crate::console::client;
use crate::console::lab_api::{self, Recipe};
use crate::console::ui::{self, Kind, Tone};
use crate::theme::{self};

/// Cards per row in the recipe grid, as the web's `minmax(280px, 1fr)` at the console's width.
const GRID_COLS: usize = 3;
const COMMIT_HINT: &str =
    "Seven to 64 hex characters of the commit, or dev-... for a local-dev pin.";

#[derive(Debug, Default)]
pub struct State {
    /// The registered recipes; None until the first answer.
    recipes: Option<Vec<Recipe>>,
    /// The last failed load's message.
    error: Option<String>,
    /// The last vet's note.
    notice: Option<String>,
    busy: bool,
    /// The open vet dialog, if any.
    vet: Option<Vet>,
    /// Whether the first load should open the vet dialog on the first recipe (the `APZ_DIALOG=vet` hook).
    capture_vet: bool,
}

/// Opens the vet dialog on the first recipe once the list has loaded (the desktop's `APZ_DIALOG=vet` hook).
pub fn capture_vet(state: &mut State) {
    state.capture_vet = true;
}

/// The vet dialog: the commit typed (prefilled with the recipe's pin), reviewed, then confirmed.
#[derive(Debug, Clone)]
struct Vet {
    id: String,
    pin: String,
    local_dev: bool,
    commit: String,
    confirming: bool,
    busy: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Vec<Recipe>, String>),
    /// Opens the vet dialog for a recipe.
    OpenVet(Recipe),
    VetCommit(String),
    VetReview,
    VetBack,
    VetConfirm,
    Vetted(Result<Recipe, String>),
    CloseVet,
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the recipes. A load still in flight is not repeated, so the tick never piles requests up.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    state.busy = true;
    let c = ctx.client.clone();
    client::task(move || lab_api::list_recipes(&c), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(result) => {
            state.busy = false;
            match result {
                Ok(list) => {
                    if state.capture_vet
                        && let Some(first) = list.first().cloned()
                    {
                        state.capture_vet = false;
                        state.vet = Some(vet_for(first));
                    }
                    state.recipes = Some(list);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::OpenVet(recipe) => {
            state.vet = Some(vet_for(recipe));
            Task::none()
        }
        Msg::VetCommit(commit) => {
            if let Some(v) = state.vet.as_mut() {
                v.commit = commit;
                v.error = None;
            }
            Task::none()
        }
        Msg::VetReview => {
            if let Some(v) = state.vet.as_mut() {
                match commit_problem(&v.commit) {
                    Some(problem) => v.error = Some(problem.to_string()),
                    None => {
                        v.error = None;
                        v.confirming = true;
                    }
                }
            }
            Task::none()
        }
        Msg::VetBack => {
            if let Some(v) = state.vet.as_mut() {
                v.confirming = false;
                v.error = None;
            }
            Task::none()
        }
        Msg::VetConfirm => {
            let Some(v) = state.vet.as_mut() else {
                return Task::none();
            };
            if v.busy {
                return Task::none();
            }
            v.busy = true;
            v.error = None;
            let c = ctx.client.clone();
            let id = v.id.clone();
            let target = v.commit.trim().to_string();
            client::task(move || lab_api::vet_recipe(&c, &id, &target), Msg::Vetted)
        }
        Msg::Vetted(Ok(done)) => {
            state.vet = None;
            state.notice = Some(format!(
                "Vetted {} at {}",
                done.id,
                short_commit(&done.commit)
            ));
            if let Some(list) = state.recipes.as_mut()
                && let Some(slot) = list.iter_mut().find(|r| r.id == done.id)
            {
                *slot = done;
            }
            Task::none()
        }
        Msg::Vetted(Err(e)) => {
            if let Some(v) = state.vet.as_mut() {
                v.busy = false;
                v.error = Some(e);
            }
            Task::none()
        }
        Msg::CloseVet => {
            state.vet = None;
            Task::none()
        }
    }
}

/// The vet dialog for a recipe: its commit prefilled with the recipe's pin, at the review step.
fn vet_for(recipe: Recipe) -> Vet {
    Vet {
        id: recipe.id,
        pin: recipe.commit.clone(),
        local_dev: recipe.local_dev,
        commit: recipe.commit,
        confirming: false,
        busy: false,
        error: None,
    }
}

/// Why a commit cannot be vetted, or None when it can: the rule apid applies (7 to 64 hex
/// characters, or `dev-` and more).
fn commit_problem(commit: &str) -> Option<&'static str> {
    let c = commit.trim();
    if c.is_empty() {
        return Some("Enter the commit to vet.");
    }
    let hex = (7..=64).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit());
    let dev = c
        .strip_prefix("dev-")
        .is_some_and(|rest| !rest.is_empty() && !rest.chars().any(char::is_whitespace));
    if hex || dev {
        None
    } else {
        Some("A commit is 7 to 64 hex characters, or dev-... for a local-dev pin.")
    }
}

/// The commit as the list shows it: its first twelve characters.
fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(note) = &state.notice {
        parts.push(lab_common::note(note.clone()));
    }
    if let Some(e) = &state.error {
        parts.push(lab_common::err(e));
    }
    match &state.recipes {
        Some(list) if list.is_empty() => parts.push(lab_common::empty(
            "flask-conical",
            "No recipes registered",
            None,
            None,
        )),
        Some(list) => parts.push(grid(list.iter().map(recipe_card).collect(), GRID_COLS)),
        None => {}
    }
    let page: Element<'a, Msg> = ui::scroll(column(parts).spacing(16).width(Length::Fill))
        .height(Length::Fill)
        .into();
    let framed = lab::frame(Section::Recipes, page, Msg::Go);
    match state.vet.as_ref() {
        Some(v) => stack(vec![framed, vet_layer(v)]).into(),
        None => framed,
    }
}

/// The cards of a grid, `cols` to a row (the web's `.lab2-grid`, 12 apart); a short last row keeps its width.
fn grid<'a>(cards: Vec<Element<'a, Msg>>, cols: usize) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut cells: Vec<Element<'a, Msg>> = Vec::new();
        for _ in 0..cols {
            match cards.next() {
                Some(card) => cells.push(container(card).width(Length::FillPortion(1)).into()),
                None => cells.push(Space::new().width(Length::FillPortion(1)).into()),
            }
        }
        rows.push(row(cells).spacing(12).width(Length::Fill).into());
    }
    column(rows).spacing(12).width(Length::Fill).into()
}

/// One recipe, as the web's card: its name on the head with the Vet action, a local-dev badge, then
/// its facts in two columns (the schema across both).
fn recipe_card<'a>(r: &'a Recipe) -> Element<'a, Msg> {
    let repo = if r.repo.is_empty() {
        "—".to_string()
    } else {
        r.repo.clone()
    };
    let facts: Vec<(bool, Element<'a, Msg>)> = vec![
        (false, fact("Id", lab_common::cell(r.id.clone(), true))),
        (false, fact("Repo", lab_common::cell(repo, true))),
        (
            false,
            fact("Commit", lab_common::cell(short_commit(&r.commit), true)),
        ),
        (false, fact("Vet", vet_line(r))),
        (
            true,
            fact("Schema", column(schema_block(&r.schema)).spacing(8).into()),
        ),
    ];
    let mut grid_rows: Vec<Element<'a, Msg>> = Vec::new();
    let mut pair: Vec<Element<'a, Msg>> = Vec::new();
    for (wide, f) in facts {
        if wide {
            if !pair.is_empty() {
                grid_rows.push(two_up(std::mem::take(&mut pair)));
            }
            grid_rows.push(f);
        } else {
            pair.push(f);
            if pair.len() == 2 {
                grid_rows.push(two_up(std::mem::take(&mut pair)));
            }
        }
    }
    if !pair.is_empty() {
        grid_rows.push(two_up(pair));
    }
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if r.local_dev {
        body.push(ui::badge("local dev", Tone::Info));
    }
    body.push(column(grid_rows).spacing(12).width(Length::Fill).into());
    ui::card_with(
        &r.name,
        vec![ui::btn_sm(
            "Vet",
            Some("check"),
            Kind::Ghost,
            Some(Msg::OpenVet(r.clone())),
        )],
        column(body).spacing(12).width(Length::Fill).into(),
    )
}

/// A two-column row of facts, each taking half the width.
fn two_up<'a>(facts: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut cells: Vec<Element<'a, Msg>> = facts
        .into_iter()
        .map(|f| container(f).width(Length::FillPortion(1)).into())
        .collect();
    while cells.len() < 2 {
        cells.push(Space::new().width(Length::FillPortion(1)).into());
    }
    row(cells).spacing(16).width(Length::Fill).into()
}

/// A fact as the web's `.lab2-fact`: its label in the small mono caps over its value.
fn fact<'a>(label: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(label), value]
        .spacing(4)
        .width(Length::Fill)
        .into()
}

/// The vet state, as the web's: a warning badge while not vetted, else who vetted it and how long ago.
fn vet_line<'a>(r: &Recipe) -> Element<'a, Msg> {
    match r.vetted_ms {
        None | Some(0) => ui::badge("not vetted", Tone::Warn),
        Some(ms) => {
            let by = r.vetted_by.as_deref().unwrap_or("unknown");
            row![
                ui::sans(
                    format!("vetted by {by} "),
                    13.0,
                    crate::theme::palette().ink
                ),
                lab_common::note(lab_common::ago_text(ms, lab_common::now_ms())),
            ]
            .align_y(Alignment::Center)
            .into()
        }
    }
}

/// The recipe's input and output schema, summarised as the web does: the feature counts with their
/// names, and the output classes. A schema of another shape is shown raw; an empty one as not declared.
fn schema_block<'a>(schema: &Value) -> Vec<Element<'a, Msg>> {
    let input = schema.get("input").and_then(Value::as_object);
    let output = schema.get("output").and_then(Value::as_object);
    if input.is_none() && output.is_none() {
        let raw = match schema.as_object() {
            Some(o) if o.is_empty() => "not declared".to_string(),
            _ => clip(&schema.to_string(), 240),
        };
        return vec![section("Schema", raw)];
    }
    let list = |obj: Option<&serde_json::Map<String, Value>>, key: &str| -> Vec<String> {
        obj.and_then(|o| o.get(key))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let numeric = list(input, "numeric");
    let categorical = list(input, "categorical");
    let mut notes: Vec<String> = Vec::new();
    if !numeric.is_empty() {
        notes.push(format!("{} numeric", numeric.len()));
    }
    if !categorical.is_empty() {
        notes.push(format!("{} categorical", categorical.len()));
    }
    if let Some(d) = input
        .and_then(|o| o.get("embedding_dim"))
        .and_then(Value::as_u64)
    {
        notes.push(format!("embedding {d}"));
    }
    if let Some(b) = input
        .and_then(|o| o.get("hash_buckets"))
        .and_then(Value::as_u64)
    {
        notes.push(format!("{} hash buckets", lab_common::count(b)));
    }
    let classes = list(output, "classes");
    let mut out = vec![section(
        "Inputs",
        if notes.is_empty() {
            "none".to_string()
        } else {
            notes.join(", ")
        },
    )];
    if !numeric.is_empty() {
        out.push(section("Numeric", numeric.join(", ")));
    }
    if !categorical.is_empty() {
        out.push(section("Categorical", categorical.join(", ")));
    }
    out.push(section(
        "Outputs",
        if classes.is_empty() {
            "none".to_string()
        } else {
            format!("{} classes", classes.len())
        },
    ));
    if !classes.is_empty() {
        out.push(section("Classes", classes.join(" · ")));
    }
    out
}

/// A schema line: its label in the small caps, its value in mono under it.
fn section<'a>(label: &str, value: String) -> Element<'a, Msg> {
    column![
        ui::label(label),
        ui::sans(value, 11.5, crate::theme::palette().ink_2)
    ]
    .spacing(4)
    .into()
}

/// `text` cut to `max` characters, with an ellipsis when it had to be cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// The vet dialog as a card over a scrim, the way the git and models screens draw their sheets.
fn vet_layer<'a>(v: &'a Vet) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if v.confirming {
        vet_confirm(v)
    } else {
        vet_edit(v)
    };
    let boxed = container(ui::card(&format!("Vet {}", v.id), body))
        .width(Length::Fixed(520.0))
        .style(theme::boxed);
    container(boxed)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(theme::scrim)
        .into()
}

/// The commit step: the field, its hint, the problem if any, then Cancel and Review.
fn vet_edit<'a>(v: &'a Vet) -> Element<'a, Msg> {
    let hint = if v.local_dev {
        format!("A local-dev recipe vets only at its own pin, {}.", v.pin)
    } else {
        COMMIT_HINT.to_string()
    };
    let input = text_input("7 to 64 hex characters, or dev-...", &v.commit)
        .on_input(Msg::VetCommit)
        .on_submit(Msg::VetReview)
        .padding(8)
        .style(theme::input);
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::label("Commit").into(),
        input.into(),
        ui::faint(hint).into(),
    ];
    if let Some(e) = &v.error {
        items.push(ui::error_line(e));
    }
    items.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CloseVet)),
            ui::btn("Review", None, Kind::Primary, Some(Msg::VetReview)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    column(items).spacing(12).into()
}

/// The confirm step: the sentence, the server's refusal if it gave one, then Back and Confirm vet.
fn vet_confirm<'a>(v: &'a Vet) -> Element<'a, Msg> {
    let sentence = format!(
        "Vet {} at commit {}? The vet records you as the vetter at this commit, so a job can use the recipe.",
        v.id,
        v.commit.trim()
    );
    let mut items: Vec<Element<'a, Msg>> = vec![ui::prose(sentence).into()];
    if let Some(e) = &v.error {
        items.push(ui::error_line(e));
    }
    items.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Back", None, Kind::Ghost, (!v.busy).then_some(Msg::VetBack)),
            ui::btn(
                "Confirm vet",
                None,
                Kind::Primary,
                (!v.busy).then_some(Msg::VetConfirm)
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    column(items).spacing(14).into()
}

/// No live topic: the web polls this screen too (every few seconds), and so does the desktop.
pub fn subscription(_ctx: &Ctx) -> iced::Subscription<Msg> {
    iced::Subscription::none()
}
