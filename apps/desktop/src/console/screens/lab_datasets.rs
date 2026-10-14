//! The Lab's datasets (`LabDatasets`), parity with the web's `#/lab/datasets`: the training sets,
//! newest first, as a table or as cards. A row opens its detail panel, with the provenance folded
//! away. Export (from the plaza's records) and upload (a JSONL file, by its path: the desktop has no
//! file picker) are dialogs; an export is confirmed before apid writes it. The list reloads on every
//! poll tick (the console's 4 s `Tick` calls `load`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use iced::widget::{button, column, container, pick_list, row, stack, text_input, Space};
use iced::widget::text::Wrapping;
use iced::{Alignment, Border, Color, Element, Length, Padding, Task, Theme};
use serde_json::Value;

use super::lab::{self, Section};
use super::lab_common;
use super::Id;
use crate::console::client;
use crate::console::lab_api::{self, Dataset, DatasetSpec};
use crate::console::ui::{self, Kind, Tone};
use crate::console::Ctx;
use crate::theme::{self};

/// Cards per row in the dataset grid, as the web's `minmax(300px, 1fr)` at the console's width.
const GRID_COLS: usize = 2;
/// The kinds an export can take, in the order the form offers them.
const KINDS: [&str; 4] = ["cascade", "tasks", "usage", "all"];
/// The label classes in the order the rung predictor reads them; any other class follows.
const CLASS_ORDER: [&str; 3] = ["cheap", "mid", "frontier"];
const TIME_HINT: &str = "YYYY-MM-DD HH:MM, local time";

/// The table or the cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Table,
    Cards,
}

#[derive(Debug, Default)]
pub struct State {
    /// The datasets; None until the first answer.
    datasets: Option<Vec<Dataset>>,
    /// The last failed load's message.
    error: Option<String>,
    /// The last export or upload's note.
    notice: Option<String>,
    busy: bool,
    view: View,
    /// The dataset whose detail panel is open.
    selected: Option<String>,
    /// Whether the open detail shows its provenance.
    provenance: bool,
    dialog: Dialog,
    /// A dialog to open once the list first loads (the capture hooks, `APZ_DIALOG`).
    capture: Option<Capture>,
}

/// A dialog a capture hook opens on the first load, prefilled.
#[derive(Debug, Clone)]
pub enum Capture {
    Export,
    Upload(Option<PathBuf>),
}

/// Opens `capture`'s dialog once the list has loaded (the desktop's `APZ_DIALOG` hook).
pub fn capture(state: &mut State, capture: Capture) {
    state.capture = Some(capture);
}

#[derive(Debug, Clone, Default)]
enum Dialog {
    #[default]
    Closed,
    Export(Export),
    Upload(Upload),
}

/// The export form: a kind, a name and an optional local time range, reviewed then confirmed.
#[derive(Debug, Clone, Default)]
struct Export {
    kind: String,
    name: String,
    since: String,
    until: String,
    confirming: bool,
    busy: bool,
    error: Option<String>,
}

/// The upload form: a name, the JSONL file the native dialog picked, and the new dataset once apid has it.
#[derive(Debug, Clone, Default)]
struct Upload {
    name: String,
    picked: Option<PathBuf>,
    busy: bool,
    error: Option<String>,
    made: Option<Dataset>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Vec<Dataset>, String>),
    SetView(View),
    /// Opens a row's detail, or closes it when it is already open.
    Select(String),
    CloseDetail,
    ToggleProvenance,
    OpenExport,
    OpenUpload,
    CloseDialog,
    ExportKind(String),
    ExportName(String),
    ExportSince(String),
    ExportUntil(String),
    ExportReview,
    ExportBack,
    ExportConfirm,
    Exported(Result<Dataset, String>),
    UploadName(String),
    /// Opens the native file dialog for the JSONL file.
    UploadChoose,
    UploadPicked(Option<PathBuf>),
    UploadSubmit,
    Uploaded(Result<Dataset, String>),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the datasets. A load still in flight is not repeated, so the tick never piles requests up.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    state.busy = true;
    let c = ctx.client.clone();
    client::task(move || lab_api::list_datasets(&c), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(result) => {
            state.busy = false;
            match result {
                Ok(list) => {
                    if state.selected.as_ref().is_some_and(|id| !list.iter().any(|d| &d.id == id)) {
                        state.selected = None;
                    }
                    state.datasets = Some(list);
                    state.error = None;
                    match state.capture.take() {
                        Some(Capture::Export) => {
                            state.dialog = Dialog::Export(Export {
                                kind: KINDS[0].to_string(),
                                name: "capture-export".to_string(),
                                since: "2026-09-01 00:00".to_string(),
                                until: "2026-09-01 06:00".to_string(),
                                ..Export::default()
                            });
                        }
                        Some(Capture::Upload(file)) => {
                            state.dialog = Dialog::Upload(Upload { name: "capture-upload".to_string(), picked: file, ..Upload::default() });
                        }
                        None => {}
                    }
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::SetView(view) => {
            state.view = view;
            Task::none()
        }
        Msg::Select(id) => {
            state.selected = if state.selected.as_deref() == Some(id.as_str()) { None } else { Some(id) };
            state.provenance = false;
            Task::none()
        }
        Msg::CloseDetail => {
            state.selected = None;
            Task::none()
        }
        Msg::ToggleProvenance => {
            state.provenance = !state.provenance;
            Task::none()
        }
        Msg::OpenExport => {
            state.dialog = Dialog::Export(Export { kind: KINDS[0].to_string(), ..Export::default() });
            Task::none()
        }
        Msg::OpenUpload => {
            state.dialog = Dialog::Upload(Upload::default());
            Task::none()
        }
        Msg::CloseDialog => {
            state.dialog = Dialog::Closed;
            Task::none()
        }
        Msg::ExportKind(kind) => {
            if let Dialog::Export(e) = &mut state.dialog {
                e.kind = kind;
                e.error = None;
            }
            Task::none()
        }
        Msg::ExportName(name) => {
            if let Dialog::Export(e) = &mut state.dialog {
                e.name = name;
                e.error = None;
            }
            Task::none()
        }
        Msg::ExportSince(since) => {
            if let Dialog::Export(e) = &mut state.dialog {
                e.since = since;
                e.error = None;
            }
            Task::none()
        }
        Msg::ExportUntil(until) => {
            if let Dialog::Export(e) = &mut state.dialog {
                e.until = until;
                e.error = None;
            }
            Task::none()
        }
        Msg::ExportReview => {
            if let Dialog::Export(e) = &mut state.dialog {
                match export_spec(e) {
                    Ok(_) => {
                        e.error = None;
                        e.confirming = true;
                    }
                    Err(problem) => e.error = Some(problem),
                }
            }
            Task::none()
        }
        Msg::ExportBack => {
            if let Dialog::Export(e) = &mut state.dialog {
                e.confirming = false;
                e.error = None;
            }
            Task::none()
        }
        Msg::ExportConfirm => {
            let Dialog::Export(e) = &mut state.dialog else { return Task::none() };
            if e.busy {
                return Task::none();
            }
            let spec = match export_spec(e) {
                Ok(spec) => spec,
                Err(problem) => {
                    e.error = Some(problem);
                    return Task::none();
                }
            };
            e.busy = true;
            e.error = None;
            let c = ctx.client.clone();
            client::task(move || lab_api::create_dataset(&c, &spec), Msg::Exported)
        }
        Msg::Exported(Ok(made)) => {
            state.dialog = Dialog::Closed;
            state.notice = Some(format!("Dataset {} made: {} rows", made.name, lab_common::count(made.rows)));
            state.selected = Some(made.id.clone());
            state.provenance = false;
            insert(state, made);
            Task::none()
        }
        Msg::Exported(Err(e)) => {
            if let Dialog::Export(x) = &mut state.dialog {
                x.busy = false;
                x.error = Some(e);
            }
            Task::none()
        }
        Msg::UploadName(name) => {
            if let Dialog::Upload(u) = &mut state.dialog {
                u.name = name;
                u.error = None;
            }
            Task::none()
        }
        Msg::UploadChoose => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a JSONL dataset")
                    .add_filter("JSONL", &["jsonl", "ndjson"])
                    .pick_file()
                    .await
                    .map(|file| file.path().to_path_buf())
            },
            Msg::UploadPicked,
        ),
        Msg::UploadPicked(file) => {
            if let (Some(path), Dialog::Upload(u)) = (file, &mut state.dialog) {
                u.picked = Some(path);
                u.error = None;
            }
            Task::none()
        }
        Msg::UploadSubmit => {
            let Dialog::Upload(u) = &mut state.dialog else { return Task::none() };
            if u.busy {
                return Task::none();
            }
            let name = u.name.trim().to_string();
            if name.is_empty() {
                u.error = Some("Enter a name for the dataset.".to_string());
                return Task::none();
            }
            let Some(path) = u.picked.clone() else {
                u.error = Some("Choose a JSONL file.".to_string());
                return Task::none();
            };
            u.busy = true;
            u.error = None;
            let c = ctx.client.clone();
            client::task(
                move || {
                    let text = std::fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
                    if let Some(problem) = jsonl_problem(&text) {
                        return Err(problem);
                    }
                    lab_api::upload_dataset(&c, &name, &text)
                },
                Msg::Uploaded,
            )
        }
        Msg::Uploaded(Ok(made)) => {
            state.notice = Some(format!("Uploaded {} as {}", made.name, made.id));
            state.selected = Some(made.id.clone());
            state.provenance = false;
            if let Dialog::Upload(u) = &mut state.dialog {
                u.busy = false;
                u.error = None;
                u.made = Some(made.clone());
            }
            insert(state, made);
            Task::none()
        }
        Msg::Uploaded(Err(e)) => {
            if let Dialog::Upload(u) = &mut state.dialog {
                u.busy = false;
                u.error = Some(e);
            }
            Task::none()
        }
    }
}

/// A new dataset joins the list at once (the next tick brings the list's own copy).
fn insert(state: &mut State, made: Dataset) {
    if let Some(list) = state.datasets.as_mut() {
        list.retain(|d| d.id != made.id);
        list.push(made);
    }
}

/// The export's request, or the reason the form cannot send it yet.
fn export_spec(e: &Export) -> Result<DatasetSpec, String> {
    let name = e.name.trim();
    if name.is_empty() {
        return Err("Enter a name for the dataset.".to_string());
    }
    let since = parse_local(&e.since, "start")?;
    let until = parse_local(&e.until, "end")?;
    if let (Some(a), Some(b)) = (since, until) {
        if a >= b {
            return Err("The start must be before the end.".to_string());
        }
    }
    Ok(DatasetSpec { kind: e.kind.clone(), name: name.to_string(), since_ms: since, until_ms: until })
}

/// A local date and time (`2026-10-13 14:05`, or with a `T`) as unix milliseconds; None when empty.
fn parse_local(value: &str, which: &str) -> Result<Option<u64>, String> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(None);
    }
    for format in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(v, format) {
            return match naive.and_local_timezone(chrono::Local).single() {
                Some(local) => Ok(Some(local.timestamp_millis().max(0) as u64)),
                None => Err(format!("The {which} is not a valid local time.")),
            };
        }
    }
    Err(format!("The {which} is not a valid date and time ({TIME_HINT})."))
}

/// Why the file is not a JSONL of objects, or None when it is; a file of no rows is a problem too.
fn jsonl_problem(text: &str) -> Option<String> {
    let mut rows = 0usize;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(_)) => rows += 1,
            Ok(_) => return Some(format!("Line {} is not a JSON object.", i + 1)),
            Err(_) => return Some(format!("Line {} is not valid JSON.", i + 1)),
        }
    }
    if rows == 0 {
        Some("The file has no rows.".to_string())
    } else {
        None
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let sorted: Option<Vec<&'a Dataset>> = state.datasets.as_ref().map(|list| {
        let mut shown: Vec<&'a Dataset> = list.iter().collect();
        shown.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
        shown
    });
    let count = match &sorted {
        Some(list) => format!("{} {}", list.len(), if list.len() == 1 { "dataset" } else { "datasets" }),
        None => String::new(),
    };
    let seg = ui::seg(&[(View::Table, "Table"), (View::Cards, "Cards")], state.view, Msg::SetView);
    let actions: Vec<Element<'a, Msg>> = vec![
        ui::faint(count).into(),
        seg,
        ui::btn("Upload", Some("file"), Kind::Secondary, Some(Msg::OpenUpload)),
        ui::btn("Export", None, Kind::Primary, Some(Msg::OpenExport)),
    ];
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::view_head("Datasets", actions)];
    if let Some(note) = &state.notice {
        parts.push(ui::faint(note.clone()).into());
    }
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(list) = &sorted {
        let open = state.selected.as_deref().and_then(|id| list.iter().copied().find(|d| d.id == id));
        if let Some(d) = open {
            parts.push(detail(d, state.provenance));
        }
        if list.is_empty() {
            parts.push(ui::empty(
                "library",
                "No datasets yet",
                "Export cascade outcomes or tasks from the plaza's records, or upload a JSONL file of rows.",
            ));
        } else {
            match state.view {
                View::Table => parts.push(table(list, state.selected.as_deref())),
                View::Cards => parts.push(grid(list.iter().copied().map(|d| dataset_card(d, state.selected.as_deref() == Some(d.id.as_str()))).collect(), GRID_COLS)),
            }
        }
    }
    let page: Element<'a, Msg> = ui::scroll(column(parts).spacing(22).width(Length::Fill)).height(Length::Fill).into();
    let framed = lab::frame(Section::Datasets, page, Msg::Go);
    match &state.dialog {
        Dialog::Closed => framed,
        Dialog::Export(e) => stack(vec![framed, dialog_layer("Export from the records", export_body(e))]).into(),
        Dialog::Upload(u) => stack(vec![framed, dialog_layer("Upload a JSONL dataset", upload_body(u))]).into(),
    }
}

/// The dialog as a card over a scrim, as the git and models screens draw their sheets.
fn dialog_layer<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let boxed = container(ui::card(title, body)).width(Length::Fixed(520.0)).style(theme::boxed);
    container(boxed)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(theme::scrim)
        .into()
}

/// The export dialog: the form, then the confirm step that says what will be written.
fn export_body<'a>(e: &'a Export) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if e.confirming {
        let (from, to) = export_spec(e).map(|s| (s.since_ms, s.until_ms)).unwrap_or((None, None));
        let range = if from.is_none() && to.is_none() {
            " over all of the records".to_string()
        } else {
            format!(
                " from {} to {} (local time)",
                from.map_or("the start".to_string(), lab_common::time),
                to.map_or("now".to_string(), lab_common::time),
            )
        };
        items.push(ui::prose(format!("Export {} records{} as {}? This writes a new dataset from the plaza.", e.kind, range, e.name.trim())).into());
        if let Some(problem) = &e.error {
            items.push(ui::error_line(problem));
        }
        items.push(
            row![
                Space::new().width(Length::Fill),
                ui::btn("Back", None, Kind::Ghost, (!e.busy).then_some(Msg::ExportBack)),
                ui::btn("Export", None, Kind::Primary, (!e.busy).then_some(Msg::ExportConfirm)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
        return column(items).spacing(14).into();
    }
    let kinds: Vec<String> = KINDS.iter().map(|k| k.to_string()).collect();
    items.push(field("Kind", pick_list(kinds, Some(e.kind.clone()), Msg::ExportKind).into()));
    items.push(field(
        "Name",
        text_input("e.g. cascade-october", &e.name).on_input(Msg::ExportName).padding(8).style(theme::input).into(),
    ));
    items.push(field(
        "From (optional)",
        text_input(TIME_HINT, &e.since).on_input(Msg::ExportSince).padding(8).style(theme::input).into(),
    ));
    items.push(field(
        "To (optional)",
        text_input(TIME_HINT, &e.until).on_input(Msg::ExportUntil).padding(8).style(theme::input).into(),
    ));
    if let Some(problem) = &e.error {
        items.push(ui::error_line(problem));
    }
    items.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Cancel", None, Kind::Ghost, Some(Msg::CloseDialog)),
            ui::btn("Review", None, Kind::Primary, Some(Msg::ExportReview)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    column(items).spacing(14).into()
}

/// The upload dialog: the form, then the new dataset on the dialog once apid has it.
fn upload_body<'a>(u: &'a Upload) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(made) = &u.made {
        items.push(ui::prose(format!("Uploaded as {}.", made.id)).into());
        items.push(
            column![
                fact("Name", ui::sans(made.name.clone(), 12.0, crate::theme::palette().ink)),
                fact("Rows", ui::sans(lab_common::count(made.rows), 12.0, crate::theme::palette().ink)),
                fact("Frozen rows", ui::sans(lab_common::count(made.frozen_rows), 12.0, crate::theme::palette().ink)),
                fact("Balance", balance_cell(made)),
                fact("Taint", taints_cell(made)),
            ]
            .spacing(8)
            .into(),
        );
        if let Some(problem) = &u.error {
            items.push(ui::error_line(problem));
        }
        items.push(
            row![Space::new().width(Length::Fill), ui::btn("Done", None, Kind::Primary, Some(Msg::CloseDialog))]
                .align_y(Alignment::Center)
                .into(),
        );
        return column(items).spacing(14).into();
    }
    items.push(field("Name", text_input("e.g. rung-labels-october", &u.name).on_input(Msg::UploadName).padding(8).style(theme::input).into()));
    let picked_name = match &u.picked {
        Some(path) => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
        None => "No file chosen.".to_string(),
    };
    items.push(field(
        "File",
        row![ui::btn("Choose file", Some("file"), Kind::Secondary, Some(Msg::UploadChoose)), ui::faint(picked_name)]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
    ));
    items.push(ui::faint("One JSON object per line. Each row needs ts (unix ms), label and the recipe features.").into());
    if let Some(problem) = &u.error {
        items.push(ui::error_line(problem));
    }
    items.push(
        row![
            Space::new().width(Length::Fill),
            ui::btn("Cancel", None, Kind::Ghost, (!u.busy).then_some(Msg::CloseDialog)),
            ui::btn(if u.busy { "Uploading" } else { "Upload" }, None, Kind::Primary, (!u.busy).then_some(Msg::UploadSubmit)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    column(items).spacing(14).into()
}

/// A dialog field: its label over its control.
fn field<'a>(title: &str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(title), control].spacing(6).into()
}

/// A label in a fixed left column over its value, as the web's definition list.
fn fact<'a>(label: &str, value: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    row![container(ui::label(label)).width(Length::Fixed(110.0)), container(value.into()).width(Length::Fill)]
        .spacing(10)
        .align_y(Alignment::Start)
        .into()
}

/// The cells of a table, by their portions of the row.
fn cell<'a>(portion: u16, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content.into()).width(Length::FillPortion(portion)).into()
}

/// The right-aligned cell of the row count.
fn cell_right<'a>(portion: u16, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content.into()).width(Length::FillPortion(portion)).align_x(Alignment::End).into()
}

/// The table's head: mono caps, the rows' own widths.
fn table<'a>(rows: &[&'a Dataset], selected: Option<&str>) -> Element<'a, Msg> {
    let heads: [(&str, u16, bool); 7] = [
        ("Name", 3, false),
        ("Kind", 2, false),
        ("Source", 2, false),
        ("Time range", 3, false),
        ("Rows", 2, true),
        ("Label balance", 3, false),
        ("Taint", 2, false),
    ];
    let head_cells: Vec<Element<'a, Msg>> = heads
        .iter()
        .map(|(label, portion, right)| {
            let content: Element<'a, Msg> = ui::sans(label.to_string(), 11.0, crate::theme::palette().ink_3).into();
            if *right { cell_right(*portion, content) } else { cell(*portion, content) }
        })
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![row(head_cells).spacing(12).padding(Padding::from([8.0, 12.0])).into(), rule()];
    for &d in rows {
        lines.push(table_row(d, selected == Some(d.id.as_str())));
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: a button over the whole row, so a click anywhere opens its detail.
fn table_row<'a>(d: &'a Dataset, selected: bool) -> Element<'a, Msg> {
    let content = row![
        cell(3, name_cell(d)),
        cell(2, ui::badge(&d.kind, kind_tone(&d.kind))),
        cell(2, ui::sans(source_of(d), 12.0, crate::theme::palette().ink_2)),
        cell(3, range_cell(d)),
        cell_right(2, rows_cell(d)),
        cell(3, balance_cell(d)),
        cell(2, taints_cell(d)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    button(content)
        .width(Length::Fill)
        .padding(Padding::from([10.0, 12.0]))
        .style(theme::register_row(selected))
        .on_press(Msg::Select(d.id.clone()))
        .into()
}

/// One dataset as a card: the same facts as the table's row, opening its detail on a click.
fn dataset_card<'a>(d: &'a Dataset, selected: bool) -> Element<'a, Msg> {
    let body = column![
        ui::mono(d.id.clone(), 11.5, crate::theme::palette().ink_3).wrapping(Wrapping::None),
        fact("Source", ui::sans(source_of(d), 12.0, crate::theme::palette().ink_2)),
        fact("Time range", range_cell(d)),
        fact("Rows", rows_cell(d)),
        fact("Balance", balance_cell(d)),
        fact("Taint", taints_cell(d)),
    ]
    .spacing(10);
    let card = ui::card(&d.name, body.into());
    button(card)
        .width(Length::Fill)
        .padding(0)
        .style(theme::register_row(selected))
        .on_press(Msg::Select(d.id.clone()))
        .into()
}

/// The detail panel of one dataset: its facts, then the provenance (folded until it is asked for).
fn detail<'a>(d: &'a Dataset, provenance_open: bool) -> Element<'a, Msg> {
    let facts = column![
        fact("Id", ui::mono(d.id.clone(), 12.0, crate::theme::palette().ink).wrapping(Wrapping::None)),
        fact("Source", ui::sans(source_of(d), 12.0, crate::theme::palette().ink_2)),
        fact("Created", ui::sans(lab_common::time(d.created_ms), 12.0, crate::theme::palette().ink_2)),
        fact("Time range", range_cell(d)),
        fact("Rows", ui::sans(lab_common::count(d.rows), 12.0, crate::theme::palette().ink)),
        fact("Frozen rows", ui::sans(lab_common::count(d.frozen_rows), 12.0, crate::theme::palette().ink)),
        fact("Balance", balance_cell(d)),
        fact("Taint", taints_cell(d)),
        fact("SHA-256", ui::mono(or_none(&d.sha256), 11.0, crate::theme::palette().ink_2)),
        fact("File", ui::mono(or_none(&d.file), 11.0, crate::theme::palette().ink_2)),
        fact("Frozen file", ui::mono(or_none(&d.frozen_file), 11.0, crate::theme::palette().ink_2)),
    ]
    .spacing(8);
    let toggle = ui::btn(if provenance_open { "Hide provenance" } else { "Show provenance" }, None, Kind::Ghost, Some(Msg::ToggleProvenance));
    let actions = row![toggle, Space::new().width(Length::Fill), ui::btn("Close", None, Kind::Ghost, Some(Msg::CloseDetail))]
        .spacing(8)
        .align_y(Alignment::Center);
    let mut body: Vec<Element<'a, Msg>> = vec![facts.into(), actions.into()];
    if provenance_open {
        let pretty = serde_json::to_string_pretty(&d.provenance).unwrap_or_default();
        body.push(ui::scroll(ui::mono(pretty, 11.0, crate::theme::palette().ink_2)).height(Length::Fixed(260.0)).into());
    }
    ui::card(&d.name, column(body).spacing(14).into())
}

/// The grid of cards, `cols` to a row; a short last row keeps its width with empty cells.
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
        rows.push(row(cells).spacing(22).into());
    }
    column(rows).spacing(22).width(Length::Fill).into()
}

/// A hairline under the table's head.
fn rule<'a>() -> Element<'a, Msg> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style { background: Some(crate::theme::palette().rule.into()), ..Default::default() })
        .into()
}

/// The dataset's name over its id; the id stays on one line.
fn name_cell<'a>(d: &'a Dataset) -> Element<'a, Msg> {
    column![
        ui::sans(d.name.clone(), 12.5, crate::theme::palette().ink),
        ui::mono(d.id.clone(), 11.0, crate::theme::palette().ink_3).wrapping(Wrapping::None),
    ]
    .spacing(3)
    .into()
}

/// The time span: from, then `to` and the end, in local time.
fn range_cell<'a>(d: &'a Dataset) -> Element<'a, Msg> {
    column![
        ui::sans(lab_common::time(d.from_ms), 11.5, crate::theme::palette().ink_2).wrapping(Wrapping::None),
        ui::sans(format!("to {}", lab_common::time(d.to_ms)), 11.5, crate::theme::palette().ink_3).wrapping(Wrapping::None),
    ]
    .spacing(2)
    .into()
}

/// The row count, with the frozen set under it (a padlock and its count) when there is one.
fn rows_cell<'a>(d: &'a Dataset) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = vec![ui::sans(lab_common::count(d.rows), 12.0, crate::theme::palette().ink).into()];
    if d.frozen_rows > 0 {
        lines.push(
            row![lock_mark(crate::theme::palette().signal_ink), ui::sans(format!("{} frozen", lab_common::count(d.frozen_rows)), 11.0, crate::theme::palette().signal_ink)]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    column(lines).align_x(Alignment::End).spacing(4).into()
}

/// A padlock drawn in two hollow rectangles: the shackle over the body.
fn lock_mark<'a>(color: Color) -> Element<'a, Msg> {
    let stroke = move |_: &Theme| container::Style { border: Border { color, width: 1.0, radius: 0.0.into() }, ..Default::default() };
    let shackle = container(Space::new()).width(Length::Fixed(5.0)).height(Length::Fixed(4.0)).style(stroke);
    let body = container(Space::new()).width(Length::Fixed(9.0)).height(Length::Fixed(6.0)).style(stroke);
    column![shackle, body].spacing(0).into()
}

/// The label balance: one segment per class, sized by its count, and a legend with the counts.
fn balance_cell<'a>(d: &'a Dataset) -> Element<'a, Msg> {
    let entries = balance_entries(&d.label_balance);
    let total: u64 = entries.iter().map(|(_, n)| *n).sum();
    if total == 0 {
        return ui::faint("no labels").into();
    }
    let segments: Vec<Element<'a, Msg>> = entries
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(class, n)| {
            let color = class_color(class);
            container(Space::new())
                .width(Length::FillPortion(portion(*n, total)))
                .height(Length::Fixed(6.0))
                .style(move |_: &Theme| container::Style { background: Some(color.into()), ..Default::default() })
                .into()
        })
        .collect();
    let bar = container(row(segments).width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(8.0))
        .style(|_: &Theme| container::Style { border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() }, ..Default::default() });
    let legend: Vec<Element<'a, Msg>> = entries
        .iter()
        .map(|(class, n)| {
            row![swatch(class_color(class)), ui::sans(format!("{class} {}", lab_common::count(*n)), 11.0, crate::theme::palette().ink_2)]
                .spacing(6)
                .align_y(Alignment::Center)
                .into()
        })
        .collect();
    column![bar, column(legend).spacing(2)].spacing(6).width(Length::Fill).into()
}

/// An 8-pixel square in a colour, for the legend.
fn swatch<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_: &Theme| container::Style { background: Some(color.into()), ..Default::default() })
        .into()
}

/// The taint flags as warn badges, or a faint none.
fn taints_cell<'a>(d: &'a Dataset) -> Element<'a, Msg> {
    if d.taint.is_empty() {
        return ui::faint("none").into();
    }
    column(d.taint.iter().map(|t| ui::badge::<Msg>(t, Tone::Warn)).collect::<Vec<_>>()).spacing(4).into()
}

/// The balance as (class, count) pairs: the known classes first, then any others by name.
fn balance_entries(balance: &BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = CLASS_ORDER.iter().filter_map(|k| balance.get(*k).map(|n| (k.to_string(), *n))).collect();
    out.extend(balance.iter().filter(|(k, _)| !CLASS_ORDER.contains(&k.as_str())).map(|(k, n)| (k.clone(), *n)));
    out
}

/// A class's colour: cheap is ok, mid info, frontier the signal; any other class is faint.
fn class_color(class: &str) -> Color {
    match class {
        "cheap" => crate::theme::palette().ok,
        "mid" => crate::theme::palette().info,
        "frontier" => crate::theme::palette().signal,
        _ => crate::theme::palette().ink_3,
    }
}

/// A class's share of the bar, as a flex portion (at least one, so a small class still shows).
fn portion(n: u64, total: u64) -> u16 {
    ((n as f64 / total as f64) * 100.0).round().clamp(1.0, 100.0) as u16
}

/// Where a dataset came from: its provenance's `source` when it names one, else what its kind says.
fn source_of(d: &Dataset) -> String {
    if let Some(s) = d.provenance.get("source").and_then(Value::as_str) {
        if !s.is_empty() {
            return s.to_string();
        }
    }
    match d.kind.as_str() {
        "upload" => "uploaded file".to_string(),
        "all" => "plaza records".to_string(),
        kind => format!("plaza {kind} records"),
    }
}

fn kind_tone(kind: &str) -> Tone {
    if kind == "upload" {
        Tone::Info
    } else {
        Tone::Muted
    }
}

fn or_none(value: &str) -> String {
    if value.is_empty() {
        "none".to_string()
    } else {
        value.to_string()
    }
}

/// No live topic: the web polls this screen too (every few seconds), and so does the desktop.
pub fn subscription(_ctx: &Ctx) -> iced::Subscription<Msg> {
    iced::Subscription::none()
}
