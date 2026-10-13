//! The Library screen (parity with the web console's `library` view and the library part of
//! `tools`): the files agents hand in (deliverables, or files they share), with a search, a type
//! filter, a preview by type, Save and Delete; and the agent-built tools, waiting for the owner's
//! review (Publish, Reject with a note), with their files and Remove.

use std::path::Path;

use iced::widget::{column, container, image, row, scrollable, text, text_input, Space};
use iced::{font, Element, Fill, Font, Length, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme;

/// Serif semibold: a card's title (the web's `.prov-title h3`).
const SEMIBOLD: Font = Font { weight: font::Weight::Semibold, ..theme::SERIF };

/// Text files up to this size are read into the detail view; larger ones are save-only.
const TEXT_LIMIT: usize = 200 * 1024;

/// A deliverable (or shared file) in the plaza's object store.
#[derive(Debug, Clone, Deserialize)]
pub struct Deliverable {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub content_type: String,
    pub size: u64,
    pub author: String,
    #[serde(default)]
    pub task: Option<String>,
    pub created_ms: i64,
    pub sha256: String,
    pub store: String,
    pub kind: String,
    pub audience: String,
    #[serde(default)]
    pub allowed: Vec<u64>,
}

/// An agent-built tool in the plaza's library.
#[derive(Debug, Clone, Deserialize)]
pub struct LibraryTool {
    pub name: String,
    pub version: u64,
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requires: Vec<String>,
    /// private | pending | published | rejected
    pub state: String,
    #[serde(default)]
    pub files: Vec<ToolFile>,
    pub created_ms: i64,
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolFile {
    pub path: String,
    pub size: u64,
}

/// Which list the screen shows: what the agents hand in, or what they share.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Deliverable,
    Shared,
}

impl Scope {
    fn param(self) -> &'static str {
        match self {
            Scope::Deliverable => "deliverable",
            Scope::Shared => "shared",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::Deliverable => "Deliverables",
            Scope::Shared => "Shared",
        }
    }

    fn intro(self) -> &'static str {
        match self {
            Scope::Deliverable => "What the agents hand in. Stored in the plaza's object store.",
            Scope::Shared => "Files the agents share with each other and on the board.",
        }
    }
}

/// The type filter's groups, as the web groups them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Video,
    Image,
    Audio,
    Documents,
    Other,
}

impl Group {
    const ALL: [Group; 5] = [Group::Video, Group::Image, Group::Audio, Group::Documents, Group::Other];

    fn of(content_type: &str) -> Group {
        let t = content_type.to_lowercase();
        if t.starts_with("video/") {
            Group::Video
        } else if t.starts_with("image/") {
            Group::Image
        } else if t.starts_with("audio/") {
            Group::Audio
        } else if t == "application/pdf"
            || textual(&t)
            || t.contains("word")
            || t.contains("officedocument")
            || t.contains("opendocument")
            || t.contains("rtf")
        {
            Group::Documents
        } else {
            Group::Other
        }
    }

    fn label(self) -> &'static str {
        match self {
            Group::Video => "Video",
            Group::Image => "Image",
            Group::Audio => "Audio",
            Group::Documents => "Documents",
            Group::Other => "Other",
        }
    }
}

/// How a file's preview is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Image,
    Markdown,
    Text,
    TooLarge,
    Nothing,
}

/// The open file's preview: loaded once when it is opened.
#[derive(Debug, Default)]
pub enum Preview {
    #[default]
    Idle,
    Loading,
    Image(image::Handle),
    Text(String),
    Markdown(String),
    Note(String),
}

/// A tool, by name and version.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRef {
    pub name: String,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolAction {
    Publish(ToolRef),
    Reject(ToolRef),
    Remove(ToolRef),
}

impl ToolAction {
    fn target(&self) -> &ToolRef {
        match self {
            ToolAction::Publish(t) | ToolAction::Reject(t) | ToolAction::Remove(t) => t,
        }
    }
}

/// One file of a tool, opened to read.
#[derive(Debug, Clone)]
pub struct ToolFileView {
    pub tool: ToolRef,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    pub tool_error: Option<String>,
    pub notice: Option<String>,
    scope: Scope,
    files: Vec<Deliverable>,
    tools: Vec<LibraryTool>,
    query: String,
    filter: Option<Group>,
    open: Option<String>,
    preview: Preview,
    confirm_delete: bool,
    tool_open: Option<ToolRef>,
    tool_view: Option<ToolFileView>,
    tool_action: Option<ToolAction>,
    reject_note: String,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Files(Result<Vec<Deliverable>, String>),
    Tools(Result<Vec<LibraryTool>, String>),
    Scope(Scope),
    Query(String),
    Filter(Option<Group>),
    Open(String),
    Close,
    PreviewLoaded(String, Result<Vec<u8>, String>),
    Save(String),
    Saved(Result<String, String>),
    AskDelete,
    CancelDelete,
    Delete(String),
    Deleted(Result<String, String>),
    ToolFiles(ToolRef),
    ToolFile(ToolRef, String),
    ToolFileLoaded(Result<ToolFileView, String>),
    ToolAsk(ToolAction),
    ToolCancel,
    ToolConfirm,
    RejectNote(String),
    Done(Result<String, String>),
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
    let files_client = ctx.client.clone();
    let path = format!("/v1/files?kind={}", state.scope.param());
    let files = client::task(move || files_client.get::<Vec<Deliverable>>(&path), Msg::Files);
    let tools_client = ctx.client.clone();
    let tools = client::task(move || tools_client.get::<Vec<LibraryTool>>("/v1/library"), Msg::Tools);
    Task::batch([files, tools])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Files(Ok(mut files)) => {
            files.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
            state.files = files;
            state.error = None;
            Task::none()
        }
        Msg::Files(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Tools(Ok(tools)) => {
            state.tools = tools;
            state.tool_error = None;
            Task::none()
        }
        Msg::Tools(Err(e)) => {
            state.tool_error = Some(e);
            Task::none()
        }
        Msg::Scope(scope) => {
            state.scope = scope;
            state.open = None;
            state.preview = Preview::Idle;
            state.confirm_delete = false;
            load(state, ctx)
        }
        Msg::Query(query) => {
            state.query = query;
            Task::none()
        }
        Msg::Filter(filter) => {
            state.filter = filter;
            Task::none()
        }
        Msg::Open(id) => open(state, id, ctx),
        Msg::Close => {
            state.open = None;
            state.preview = Preview::Idle;
            state.confirm_delete = false;
            Task::none()
        }
        Msg::PreviewLoaded(id, result) => {
            preview_loaded(state, &id, result);
            Task::none()
        }
        Msg::Save(id) => save(state, &id, ctx),
        Msg::Saved(Ok(path)) => {
            state.notice = Some(format!("Saved to {path}"));
            Task::none()
        }
        Msg::Saved(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::AskDelete => {
            state.confirm_delete = true;
            Task::none()
        }
        Msg::CancelDelete => {
            state.confirm_delete = false;
            Task::none()
        }
        Msg::Delete(id) => delete(id, ctx),
        Msg::Deleted(Ok(id)) => {
            state.files.retain(|d| d.id != id);
            if state.open.as_deref() == Some(id.as_str()) {
                state.open = None;
                state.preview = Preview::Idle;
            }
            state.confirm_delete = false;
            state.notice = Some("File deleted".into());
            load(state, ctx)
        }
        Msg::Deleted(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::ToolFiles(tool) => {
            if state.tool_open.as_ref() == Some(&tool) {
                state.tool_open = None;
            } else {
                state.tool_open = Some(tool);
            }
            state.tool_view = None;
            Task::none()
        }
        Msg::ToolFile(tool, path) => tool_file(tool, path, ctx),
        Msg::ToolFileLoaded(Ok(view)) => {
            state.tool_view = Some(view);
            Task::none()
        }
        Msg::ToolFileLoaded(Err(e)) => {
            state.tool_error = Some(e);
            Task::none()
        }
        Msg::ToolAsk(action) => tool_ask(state, action, ctx),
        Msg::ToolCancel => {
            state.tool_action = None;
            state.reject_note.clear();
            Task::none()
        }
        Msg::ToolConfirm => match state.tool_action.take() {
            Some(action) => {
                let note = std::mem::take(&mut state.reject_note);
                run_tool(ctx, action, note)
            }
            None => Task::none(),
        },
        Msg::RejectNote(note) => {
            state.reject_note = note;
            Task::none()
        }
        Msg::Done(Ok(label)) => {
            state.notice = Some(label);
            state.tool_action = None;
            state.tool_open = None;
            state.tool_view = None;
            state.reject_note.clear();
            load(state, ctx)
        }
        Msg::Done(Err(e)) => {
            state.tool_error = Some(e);
            state.tool_action = None;
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        items.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        items.push(ui::faint(n.as_str()).into());
    }
    let open = state.open.as_deref().and_then(|id| state.files.iter().find(|d| d.id == id));
    match open {
        Some(d) => items.push(detail(state, d)),
        None => list(state, &mut items),
    }
    ui::sheet(ui::view_head("Library", vec![]), scrollable(column(items).spacing(16).width(Fill)).into())
}

// ---- Files ----------------------------------------------------------------------------------

/// Opens a file: loads its preview if the type has one, else says so.
fn open(state: &mut State, id: String, ctx: &Ctx) -> Task<Msg> {
    let (how, size) = match state.files.iter().find(|d| d.id == id) {
        Some(d) => (shown(d), d.size),
        None => return Task::none(),
    };
    state.open = Some(id.clone());
    state.notice = None;
    state.confirm_delete = false;
    match how {
        Shown::Image | Shown::Markdown | Shown::Text => {
            state.preview = Preview::Loading;
            fetch_preview(ctx, id)
        }
        Shown::TooLarge => {
            state.preview = Preview::Note(format!("Too large to preview ({}). Save it to read it.", human_size(size)));
            Task::none()
        }
        Shown::Nothing => {
            state.preview = Preview::Note("No preview for this type. Save it to open it.".into());
            Task::none()
        }
    }
}

fn fetch_preview(ctx: &Ctx, id: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/files/{id}/content");
    client::task(move || fetch_bytes(&client, &path), move |r| Msg::PreviewLoaded(id.clone(), r))
}

fn preview_loaded(state: &mut State, id: &str, result: Result<Vec<u8>, String>) {
    if state.open.as_deref() != Some(id) {
        return;
    }
    let Some(d) = state.files.iter().find(|d| d.id == id) else {
        state.preview = Preview::Idle;
        return;
    };
    state.preview = match result {
        Err(_) => Preview::Note("Could not read this file.".into()),
        Ok(bytes) => match shown(d) {
            Shown::Image => Preview::Image(image::Handle::from_bytes(bytes)),
            Shown::Markdown => Preview::Markdown(clip(&bytes)),
            Shown::Text => Preview::Text(clip(&bytes)),
            Shown::TooLarge | Shown::Nothing => Preview::Idle,
        },
    };
}

fn save(state: &State, id: &str, ctx: &Ctx) -> Task<Msg> {
    let Some(name) = state.files.iter().find(|d| d.id == id).map(|d| d.name.clone()) else {
        return Task::none();
    };
    let client = ctx.client.clone();
    let path = format!("/v1/files/{id}/content?download=1");
    client::task(
        move || {
            let bytes = fetch_bytes(&client, &path)?;
            save_download(&name, &bytes)
        },
        Msg::Saved,
    )
}

fn delete(id: String, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/files/{id}");
    client::task(move || client.delete(&path).map(|_| id), Msg::Deleted)
}

fn list<'a>(state: &'a State, items: &mut Vec<Element<'a, Msg>>) {
    let tabs: Vec<Element<'a, Msg>> = [Scope::Deliverable, Scope::Shared]
        .into_iter()
        .map(|scope| ui::btn(scope.label(), None, kind_for(state.scope == scope), Some(Msg::Scope(scope))))
        .collect();
    items.push(row(tabs).spacing(8).into());
    items.push(ui::faint(state.scope.intro()).into());

    let search = text_input("Search by title or name", &state.query).on_input(Msg::Query).style(theme::input).padding(8);
    let mut filters: Vec<Element<'a, Msg>> =
        vec![ui::btn("All", None, kind_for(state.filter.is_none()), Some(Msg::Filter(None)))];
    for group in Group::ALL {
        filters.push(ui::btn(group.label(), None, kind_for(state.filter == Some(group)), Some(Msg::Filter(Some(group)))));
    }
    items.push(column![search, row(filters).spacing(6)].spacing(8).into());

    if state.files.is_empty() {
        items.push(ui::empty("board", "Nothing handed in yet", "Agents deliver finished work with deliver_file."));
    } else {
        let shown_files: Vec<&'a Deliverable> = state.files.iter().filter(|d| matches(state, d)).collect();
        if shown_files.is_empty() {
            items.push(ui::faint("Nothing matches that search.").into());
        }
        for chunk in shown_files.chunks(3) {
            let mut cells: Vec<Element<'a, Msg>> = chunk.iter().map(|d| card(*d)).collect();
            while cells.len() < 3 {
                cells.push(Space::new().width(Fill).into());
            }
            items.push(row(cells).spacing(12).into());
        }
    }

    items.push(ui::label("Agent tools").into());
    if let Some(e) = &state.tool_error {
        items.push(ui::error_line(e));
    }
    if state.tools.is_empty() {
        items.push(ui::empty(
            "models",
            "No tools yet",
            "Agents can build tools in their VM and package them with tool_package; they show up here.",
        ));
        return;
    }
    let (pending, rest): (Vec<&'a LibraryTool>, Vec<&'a LibraryTool>) =
        state.tools.iter().partition(|t| t.state == "pending");
    if !pending.is_empty() {
        items.push(ui::label("Waiting for your review").into());
        for tool in pending {
            items.push(tool_card(state, tool));
        }
    }
    if !rest.is_empty() {
        items.push(ui::label("Library").into());
        for tool in rest {
            items.push(tool_card(state, tool));
        }
    }
}

fn matches(state: &State, d: &Deliverable) -> bool {
    let q = state.query.trim().to_lowercase();
    state.filter.map_or(true, |g| g == Group::of(&d.content_type))
        && (q.is_empty() || d.title.to_lowercase().contains(&q) || d.name.to_lowercase().contains(&q))
}

/// One file's card: its tile (the monogram of its extension), title, name, and who and when.
fn card<'a>(d: &'a Deliverable) -> Element<'a, Msg> {
    let title = if d.title.is_empty() { d.name.as_str() } else { d.title.as_str() };
    let mut body: Vec<Element<'a, Msg>> = vec![
        container(ui::faint(d.store.as_str())).into(),
        container(ui::mono(monogram(&d.name), 20.0, crate::theme::palette().ink_2)).padding(12).width(Fill).into(),
        text(title).font(SEMIBOLD).size(15.0).color(crate::theme::palette().ink).into(),
        ui::mono(d.name.as_str(), 11.0, crate::theme::palette().ink_3).into(),
        ui::faint(format!("by {} · {} · {}", d.author, ago(d.created_ms), human_size(d.size))).into(),
    ];
    if d.kind == "shared" && d.audience == "private" {
        body.push(ui::faint(format!("private · {} agents", d.allowed.len())).into());
    }
    if let Some(task) = &d.task {
        body.push(ui::mono(task.as_str(), 11.0, crate::theme::palette().ink_3).into());
    }
    button_card(body, Msg::Open(d.id.clone()))
}

fn button_card<'a>(body: Vec<Element<'a, Msg>>, on_press: Msg) -> Element<'a, Msg> {
    iced::widget::button(column(body).spacing(6).width(Fill))
        .on_press(on_press)
        .style(theme::register_row(false))
        .padding(12)
        .width(Fill)
        .into()
}

/// The open file, in place of the list: its preview, facts, description, Save and Delete.
fn detail<'a>(state: &'a State, d: &'a Deliverable) -> Element<'a, Msg> {
    let title = if d.title.is_empty() { d.name.as_str() } else { d.title.as_str() };
    let preview: Element<'a, Msg> = match &state.preview {
        Preview::Idle => ui::faint("").into(),
        Preview::Loading => ui::loading(),
        Preview::Image(handle) => image(handle.clone()).width(Fill).into(),
        Preview::Text(s) => container(ui::mono(s.as_str(), 12.0, crate::theme::palette().ink)).padding(12).width(Fill).into(),
        Preview::Markdown(s) => ui::markdown(s),
        Preview::Note(s) => ui::faint(s.as_str()).into(),
    };
    let mut body: Vec<Element<'a, Msg>> = vec![preview, facts(d)];
    if let Some(desc) = &d.description {
        body.push(ui::faint(desc.as_str()).into());
    }
    body.push(file_actions(state, d));
    column![
        ui::btn("Back to the library", None, ui::Kind::Ghost, Some(Msg::Close)),
        ui::card(title, column(body).spacing(12).into()),
    ]
    .spacing(12)
    .into()
}

fn file_actions<'a>(state: &'a State, d: &'a Deliverable) -> Element<'a, Msg> {
    if state.confirm_delete {
        column![
            ui::faint("Removes the file from the plaza's object store. This cannot be undone."),
            row![
                Space::new().width(Fill),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDelete)),
                ui::btn("Delete", None, ui::Kind::Primary, Some(Msg::Delete(d.id.clone()))),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .into()
    } else {
        row![
            Space::new().width(Fill),
            ui::btn("Delete", None, ui::Kind::Ghost, Some(Msg::AskDelete)),
            ui::btn("Download", None, ui::Kind::Primary, Some(Msg::Save(d.id.clone()))),
        ]
        .spacing(8)
        .into()
    }
}

fn facts<'a>(d: &'a Deliverable) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = vec![
        fact("Name", d.name.clone(), true),
        fact("Content type", d.content_type.clone(), true),
        fact("Size", format!("{} ({} bytes)", human_size(d.size), d.size), false),
        fact("sha256", d.sha256.clone(), true),
        fact("Handed in by", d.author.clone(), false),
        fact("When", when(d.created_ms), false),
        fact("Store", d.store.clone(), false),
    ];
    if let Some(task) = &d.task {
        rows.push(fact("Task", task.clone(), true));
    }
    column(rows).spacing(6).into()
}

/// One fact: a faint label, then the value (mono for names, hashes and ids; serif otherwise).
fn fact<'a>(label: &'static str, value: String, mono: bool) -> Element<'a, Msg> {
    let value: Element<'a, Msg> = if mono { ui::mono(value, 12.0, crate::theme::palette().ink).into() } else { ui::prose(value).size(13.0).into() };
    row![container(ui::faint(label)).width(Length::Fixed(120.0)), value].spacing(8).into()
}

// ---- Library tools --------------------------------------------------------------------------

fn tool_card<'a>(state: &'a State, t: &'a LibraryTool) -> Element<'a, Msg> {
    let me = ToolRef { name: t.name.clone(), version: t.version };
    let mut body: Vec<Element<'a, Msg>> = vec![ui::faint(format!(
        "v{} · by {} · {} · {} calls",
        t.version,
        t.author,
        ago(t.created_ms),
        t.calls
    ))
    .into()];
    if !t.description.is_empty() {
        body.push(ui::prose(t.description.as_str()).into());
    }
    if !t.requires.is_empty() {
        body.push(ui::faint(format!("Needs {}", t.requires.join(", "))).into());
    }
    if let Some(note) = &t.review_note {
        body.push(ui::faint(format!("Note: {note}")).into());
    }

    let mut bar: Vec<Element<'a, Msg>> = vec![Space::new().width(Fill).into()];
    if t.state == "pending" {
        bar.push(ui::btn("Publish", None, ui::Kind::Primary, Some(Msg::ToolAsk(ToolAction::Publish(me.clone())))));
        bar.push(ui::btn("Reject", None, ui::Kind::Ghost, Some(Msg::ToolAsk(ToolAction::Reject(me.clone())))));
    }
    if !t.files.is_empty() {
        bar.push(ui::btn("Files", None, ui::Kind::Ghost, Some(Msg::ToolFiles(me.clone()))));
    }
    bar.push(ui::btn("Remove", None, ui::Kind::Ghost, Some(Msg::ToolAsk(ToolAction::Remove(me.clone())))));
    body.push(row(bar).spacing(8).into());

    match state.tool_action.as_ref().filter(|a| a.target() == &me) {
        Some(ToolAction::Reject(_)) => {
            body.push(
                text_input("Why it is rejected (short)", &state.reject_note)
                    .on_input(Msg::RejectNote)
                    .style(theme::input)
                    .padding(8)
                    .into(),
            );
            body.push(confirm_row("Reject"));
        }
        Some(ToolAction::Remove(_)) => {
            body.push(
                ui::faint(format!("Removes v{} and its files from the library. This cannot be undone.", t.version)).into(),
            );
            body.push(confirm_row("Remove"));
        }
        _ => {}
    }

    if state.tool_open.as_ref() == Some(&me) {
        for f in &t.files {
            let label = format!("{} · {}", f.path, human_size(f.size));
            body.push(ui::btn(&label, None, ui::Kind::Ghost, Some(Msg::ToolFile(me.clone(), f.path.clone()))));
        }
        if let Some(view) = state.tool_view.as_ref().filter(|v| v.tool == me) {
            body.push(ui::faint(view.path.as_str()).into());
            body.push(container(ui::mono(view.content.as_str(), 12.0, crate::theme::palette().ink)).padding(12).width(Fill).into());
        }
    }

    ui::card(&format!("x_{}", t.name), column(body).spacing(8).into())
}

fn confirm_row<'a>(label: &str) -> Element<'a, Msg> {
    row![
        Space::new().width(Fill),
        ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::ToolCancel)),
        ui::btn(label, None, ui::Kind::Primary, Some(Msg::ToolConfirm)),
    ]
    .spacing(8)
    .into()
}

fn tool_file(tool: ToolRef, path: String, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let url = format!("{}/files?path={}", tool_path(&tool), enc(&path));
    client::task(
        move || {
            let value = client.get::<Value>(&url)?;
            let content = value.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
            Ok(ToolFileView { tool, path, content })
        },
        Msg::ToolFileLoaded,
    )
}

/// Publish runs at once; Reject and Remove wait for the owner's confirmation.
fn tool_ask(state: &mut State, action: ToolAction, ctx: &Ctx) -> Task<Msg> {
    if let ToolAction::Publish(_) = action {
        return run_tool(ctx, action, String::new());
    }
    state.reject_note.clear();
    state.tool_action = Some(action);
    Task::none()
}

fn run_tool(ctx: &Ctx, action: ToolAction, note: String) -> Task<Msg> {
    let client = ctx.client.clone();
    match action {
        ToolAction::Publish(tool) => review(client, tool, true, None),
        ToolAction::Reject(tool) => review(client, tool, false, Some(note).filter(|n| !n.trim().is_empty())),
        ToolAction::Remove(tool) => {
            let label = format!("x_{} removed", tool.name);
            let path = tool_path(&tool);
            client::task(move || client.delete(&path).map(|_| label), Msg::Done)
        }
    }
}

fn review(client: client::Client, tool: ToolRef, approve: bool, note: Option<String>) -> Task<Msg> {
    let verb = if approve { "published" } else { "rejected" };
    let label = format!("x_{} {verb}", tool.name);
    let mut body = json!({ "approve": approve });
    if let Some(note) = note {
        body["note"] = json!(note.trim());
    }
    let path = format!("{}/review", tool_path(&tool));
    client::task(move || client.post::<Value>(&path, &body).map(|_| label), Msg::Done)
}

// ---- Helpers --------------------------------------------------------------------------------

fn kind_for(on: bool) -> ui::Kind {
    if on {
        ui::Kind::Primary
    } else {
        ui::Kind::Ghost
    }
}

fn textual(content_type: &str) -> bool {
    let t = content_type.to_lowercase();
    t.starts_with("text/") || t.contains("json") || t.contains("xml")
}

fn shown(d: &Deliverable) -> Shown {
    if Group::of(&d.content_type) == Group::Image {
        Shown::Image
    } else if !textual(&d.content_type) {
        Shown::Nothing
    } else if d.size > TEXT_LIMIT as u64 {
        Shown::TooLarge
    } else if d.name.to_lowercase().ends_with(".md") || d.content_type.eq_ignore_ascii_case("text/markdown") {
        Shown::Markdown
    } else {
        Shown::Text
    }
}

/// Clips text to the preview limit on a character boundary.
fn clip(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= TEXT_LIMIT {
        return text.into_owned();
    }
    let mut end = TEXT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…", &text[..end])
}

/// The extension in capitals, for a file with no preview of its own.
fn monogram(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.chars().take(4).collect::<String>().to_uppercase(),
        _ => "?".into(),
    }
}

fn human_size(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n < KB {
        format!("{n} B")
    } else if n < MB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else if n < GB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else {
        format!("{:.1} GB", n as f64 / GB as f64)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ago(ms: i64) -> String {
    let s = ((now_ms() - ms) / 1000).max(0);
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86_400)
    }
}

/// The instant as a UTC date and time (civil date from days since the epoch).
fn when(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

fn tool_path(tool: &ToolRef) -> String {
    format!("/v1/library/{}/{}", enc(&tool.name), tool.version)
}

/// Percent-encodes everything but unreserved characters (for path segments and query values).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The raw bytes of a hub path (the file's content), or the hub's error.
fn fetch_bytes(client: &client::Client, path: &str) -> Result<Vec<u8>, String> {
    let reply = apz_http::client::request(&client.target, "GET", path, None, client.timeout).map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}", reply.status))
    }
}

/// Writes a download to ~/Downloads under the file's own name (never a path from the name).
fn save_download(name: &str, bytes: &[u8]) -> Result<String, String> {
    let dir = apz_os::dirs::home().ok_or_else(|| "the home directory is not set".to_string())?.join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("deliverable");
    let target = dir.join(file);
    std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
    Ok(target.display().to_string())
}
