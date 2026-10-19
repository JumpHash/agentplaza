//! The Library screen (parity with the web console's `library` view and the library part of
//! `tools`): the files agents hand in (deliverables, or files they share), with a search, a type
//! filter, a preview by type, Save and Delete; and the agent-built tools, waiting for the owner's
//! review (Publish, Reject with a note), with their files and Remove.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use iced::widget::{
    Space, button, column, container, image, pick_list, responsive, row, stack, text, text_input,
};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Font, Length, Shadow, Size, Task, Theme,
    font,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::artifact::Sharing;
use crate::console::{Ctx, client, events, share, ui};
use crate::theme;

/// Serif semibold: a card's title (the web's `.prov-title h3`).
const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..theme::SANS
};

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
            Scope::Deliverable => "Files agents hand in when their work is finished.",
            Scope::Shared => "Files the owner shares with the plaza.",
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
    const ALL: [Group; 5] = [
        Group::Video,
        Group::Image,
        Group::Audio,
        Group::Documents,
        Group::Other,
    ];

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
    /// A video: no inline player in iced, so a Play action hands it to the system's player.
    Video,
    /// A PDF: no inline viewer in iced, so an Open action hands it to the system's viewer.
    Pdf,
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
    /// The type select picks the artifacts: the grid lists them instead of the files.
    artifacts_on: bool,
    open: Option<String>,
    preview: Preview,
    confirm_delete: bool,
    /// The delete dialog's title, "Delete <name>?", set when the owner asks.
    delete_title: String,
    tool_open: Option<ToolRef>,
    tool_view: Option<ToolFileView>,
    tool_action: Option<ToolAction>,
    reject_note: String,
    /// The file last picked (opened by a click or by Enter): Enter opens it again from the list.
    selected: Option<String>,
    /// The file the arrow keys picked, as its index in the list the screen shows (see `visible`).
    kbd: Option<usize>,
    /// The artifacts the Artifacts group lists.
    artifacts: Vec<apz_view::artifact::Summary>,
    artifact_error: Option<String>,
    /// The Share dialog, while it is open over the screen.
    share: Option<Sharing>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Files(Result<Vec<Deliverable>, String>),
    Tools(Result<Vec<LibraryTool>, String>),
    /// The artifacts the agents made (the Artifacts group), newest first.
    Artifacts(Result<Vec<apz_view::artifact::Summary>, String>),
    Scope(Scope),
    Query(String),
    Type(Choice),
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
    /// Enter pressed while no text field has the key: opens the file the arrows picked, else the selected one.
    OpenSelected,
    /// An arrow key pressed: true for down, false for up. Moves the pick; opens nothing.
    Step(bool),
    /// Play a video or open a PDF: save it and hand it to the system's app.
    Play(String),
    Played(Result<String, String>),
    /// A hub event: a file was handed in, shared or removed, so the list reloads.
    Event(events::Event),
    /// The file detail's Share press: opens the share dialog for the file, by its id and name.
    ShareOpen(String, String),
    /// A message from the share dialog.
    Share(share::Msg),
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
    let files = client::task(
        move || files_client.get::<Vec<Deliverable>>(&path),
        Msg::Files,
    );
    let tools_client = ctx.client.clone();
    let tools = client::task(
        move || tools_client.get::<Vec<LibraryTool>>("/v1/library"),
        Msg::Tools,
    );
    let artifacts_client = ctx.client.clone();
    let artifacts = client::task(
        move || artifacts_client.get::<Vec<apz_view::artifact::Summary>>("/v1/artifacts"),
        Msg::Artifacts,
    );
    Task::batch([files, tools, artifacts])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Files(Ok(mut files)) => {
            files.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
            state.files = files;
            state.error = None;
            clamp_kbd(state);
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
        Msg::Artifacts(Ok(artifacts)) => {
            state.artifacts = artifacts;
            state.artifact_error = None;
            Task::none()
        }
        Msg::Artifacts(Err(e)) => {
            state.artifact_error = Some(e);
            Task::none()
        }
        Msg::Scope(scope) => {
            state.scope = scope;
            state.open = None;
            state.preview = Preview::Idle;
            state.confirm_delete = false;
            state.kbd = None;
            load(state, ctx)
        }
        Msg::Query(query) => {
            state.query = query;
            state.kbd = None;
            Task::none()
        }
        Msg::Type(choice) => {
            state.artifacts_on = choice == Choice::Artifacts;
            state.filter = match choice {
                Choice::Group(group) => Some(group),
                _ => None,
            };
            state.kbd = None;
            Task::none()
        }
        Msg::OpenSelected => {
            // The list is hidden while a file is open: Enter does nothing then.
            if state.open.is_some() {
                return Task::none();
            }
            let picked = state
                .kbd
                .and_then(|i| visible(state).get(i).map(|d| d.id.clone()));
            match picked.or_else(|| state.selected.clone()) {
                Some(id) => open(state, id, ctx),
                None => Task::none(),
            }
        }
        Msg::Step(down) => {
            if state.open.is_none() {
                let len = visible(state).len();
                state.kbd = step(state.kbd, len, down);
            }
            Task::none()
        }
        Msg::Play(id) => play(state, &id, ctx),
        Msg::Event(event) => {
            if event.kind == "file" {
                load(state, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Played(Ok(path)) => {
            state.notice = Some(format!("Opened {path}"));
            Task::none()
        }
        Msg::Played(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::ShareOpen(id, name) => {
            let (dialog, task) = share::open(&ctx.client, "file", &id, &name, None);
            state.share = Some(Sharing(dialog));
            task.map(Msg::Share)
        }
        Msg::Share(msg) => {
            let Some(sharing) = state.share.as_mut() else {
                return Task::none();
            };
            let (task, closed) = share::update(&mut sharing.0, msg);
            if closed {
                state.share = None;
            }
            task.map(Msg::Share)
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
            let name = state
                .open
                .as_deref()
                .and_then(|id| state.files.iter().find(|d| d.id == id))
                .map(|d| {
                    if d.title.is_empty() {
                        d.name.as_str()
                    } else {
                        d.title.as_str()
                    }
                })
                .unwrap_or_default()
                .to_string();
            state.delete_title = format!("Delete {name}?");
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
            clamp_kbd(state);
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

/// Enter opens the picked file (else the selected one), and the arrows move the pick, but only when no
/// widget captured the key. iced's text input captures Enter only with a submit handler and never the
/// arrows, so the arrows still reach here while the search field has focus; they do not edit its text.
/// The hub's `file` events reload the list, as the web's Library does.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    use iced::keyboard::key::Named;
    let keys = iced::event::listen_with(|event, status, _window| match (event, status) {
        (
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(Named::Enter),
                ..
            }),
            iced::event::Status::Ignored,
        ) => Some(Msg::OpenSelected),
        (
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(key @ (Named::ArrowDown | Named::ArrowUp)),
                ..
            }),
            iced::event::Status::Ignored,
        ) => Some(Msg::Step(key == Named::ArrowDown)),
        _ => None,
    });
    iced::Subscription::batch([keys, events::subscription(&ctx.client).map(Msg::Event)])
}

/// The type filter's choices: every type, one group, or the artifacts the agents made (the web's type select).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    All,
    Group(Group),
    Artifacts,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Choice::All => "All",
            Choice::Group(g) => g.label(),
            Choice::Artifacts => "Artifacts",
        })
    }
}

/// A tile is at least this wide, with this gap between tiles (the web's `.lib-grid`).
const TILE_MIN: f32 = 200.0;
const TILE_GAP: f32 = 12.0;
/// The open file's panel beside the grid (the web's `.lib-cols.has-sel`).
const ASIDE_WIDTH: f32 = 380.0;
/// A tile's preview height (the web's `.lib-card-thumb`).
const THUMB_HEIGHT: f32 = 96.0;
/// The controls' text size (the search and the type select).
const CONTROL: f32 = 13.0;

/// The screen: the library's page, with the Share dialog stacked over it while it is open.
pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let page = ui::with_modal(page(state, ctx), delete_dialog(state));
    match &state.share {
        Some(sharing) => {
            iced::widget::stack([page, share::view(&sharing.0).map(Msg::Share)]).into()
        }
        None => page,
    }
}

/// The delete dialog over the screen while the owner has asked to delete the open file (the web's confirm): what it
/// removes, and Cancel and Delete in the foot.
fn delete_dialog(state: &State) -> Option<Element<'_, Msg>> {
    if !state.confirm_delete || state.artifacts_on {
        return None;
    }
    let d = state
        .open
        .as_deref()
        .and_then(|id| state.files.iter().find(|f| f.id == id))?;
    let body = ui::faint("Removes the file from the plaza's object store. This cannot be undone.");
    Some(ui::modal(
        state.delete_title.as_str(),
        body.into(),
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDelete)),
            ui::btn(
                "Delete",
                None,
                ui::Kind::Primary,
                Some(Msg::Delete(d.id.clone())),
            ),
        ],
        Msg::CancelDelete,
        440.0,
    ))
}

/// The library's page, as the web's Library: the head, the Deliverables and Shared tabs over their intro, the search
/// and the type filter, the grid of tiles and, beside it, the open file's panel. The agent tools follow the grid.
fn page<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        items.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        items.push(ui::faint(n.as_str()).into());
    }
    let open = if state.artifacts_on {
        None
    } else {
        state
            .open
            .as_deref()
            .and_then(|id| state.files.iter().find(|d| d.id == id))
    };
    let main = column![
        scope_tabs(state),
        ui::faint(state.scope.intro()),
        filters(state),
        grid(state),
    ]
    .spacing(12)
    .width(Fill);
    let body: Element<'a, Msg> = match open {
        Some(d) => row![
            container(main).width(Fill),
            container(aside(state, d)).width(Length::Fixed(ASIDE_WIDTH)),
        ]
        .spacing(16)
        .align_y(Alignment::Start)
        .into(),
        None => main.into(),
    };
    items.push(body);
    tools(state, &mut items);
    ui::sheet(
        ui::view_head_with_intro(
            "Library",
            "Files agents hand in, and the files the owner shares.",
            Vec::new(),
        ),
        ui::scroll(column(items).spacing(16).width(Fill)).into(),
    )
}

/// The Deliverables and Shared tabs; the count of the tab on show follows its name.
fn scope_tabs<'a>(state: &'a State) -> Element<'a, Msg> {
    let count = state.files.len();
    let deliverables = if state.scope == Scope::Deliverable {
        format!("{} {count}", Scope::Deliverable.label())
    } else {
        Scope::Deliverable.label().to_string()
    };
    let shared = if state.scope == Scope::Shared {
        format!("{} {count}", Scope::Shared.label())
    } else {
        Scope::Shared.label().to_string()
    };
    ui::tabs(
        &[
            (Scope::Deliverable, deliverables.as_str()),
            (Scope::Shared, shared.as_str()),
        ],
        state.scope,
        Msg::Scope,
    )
}

/// The search (by title or name) and the type select, on one line.
fn filters<'a>(state: &'a State) -> Element<'a, Msg> {
    let search = text_input("Search by title or name", &state.query)
        .on_input(Msg::Query)
        .style(theme::input)
        .padding(8)
        .width(Fill);
    let mut options: Vec<Choice> = vec![Choice::All];
    options.extend(Group::ALL.into_iter().map(Choice::Group));
    options.push(Choice::Artifacts);
    let picked = if state.artifacts_on {
        Choice::Artifacts
    } else {
        state.filter.map_or(Choice::All, Choice::Group)
    };
    let kind = pick_list(options, Some(picked), Msg::Type)
        .style(select_style)
        .text_size(CONTROL)
        .font(theme::SANS)
        .width(Length::Fixed(120.0));
    row![search, kind]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// The web's `.select`: a transparent field with an ink rule, the handle in ink-2.
fn select_style(_: &Theme, _: pick_list::Status) -> pick_list::Style {
    let p = crate::theme::palette();
    pick_list::Style {
        text_color: p.ink,
        placeholder_color: p.ink_3,
        handle_color: p.ink_2,
        background: Background::Color(p.raised),
        border: Border {
            color: p.rule_strong,
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
    }
}

/// The grid: the files the tab and the filter leave, or the artifacts; as many tiles a row as the width takes.
/// The empty states are the web's: nothing handed in, no artifacts yet, or nothing matches the search.
fn grid<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.artifacts_on {
        if state.artifacts.is_empty() {
            return ui::empty(
                "library",
                "No artifacts yet",
                "Agents make them with artifact_create.",
            );
        }
    } else if state.files.is_empty() {
        return ui::empty(
            "library",
            "Nothing handed in yet.",
            "Agents deliver finished work with deliver_file.",
        );
    }
    responsive(move |size: Size| -> Element<'a, Msg> {
        let cells = if state.artifacts_on {
            artifact_tiles(state)
        } else {
            file_tiles(state)
        };
        if cells.is_empty() {
            return ui::empty("search", "Nothing matches that search.", "");
        }
        let per_row = ((size.width + TILE_GAP) / (TILE_MIN + TILE_GAP))
            .floor()
            .max(1.0) as usize;
        let mut rows = column(Vec::<Element<'a, Msg>>::new())
            .spacing(TILE_GAP)
            .width(Fill);
        let mut line = row(Vec::<Element<'a, Msg>>::new())
            .spacing(TILE_GAP)
            .width(Fill);
        let mut n = 0;
        for cell in cells {
            line = line.push(container(cell).width(Fill));
            n += 1;
            if n == per_row {
                rows = rows.push(line);
                line = row(Vec::<Element<'a, Msg>>::new())
                    .spacing(TILE_GAP)
                    .width(Fill);
                n = 0;
            }
        }
        if n > 0 {
            while n < per_row {
                line = line.push(Space::new().width(Fill));
                n += 1;
            }
            rows = rows.push(line);
        }
        rows.into()
    })
    .into()
}

fn open(state: &mut State, id: String, ctx: &Ctx) -> Task<Msg> {
    state.selected = Some(id.clone());
    let video = state
        .files
        .iter()
        .any(|d| d.id == id && Group::of(&d.content_type) == Group::Video);
    let pdf = state
        .files
        .iter()
        .any(|d| d.id == id && is_pdf(&d.content_type));
    if video || pdf {
        state.open = Some(id);
        state.notice = None;
        state.confirm_delete = false;
        state.preview = if video { Preview::Video } else { Preview::Pdf };
        return Task::none();
    }
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
            state.preview = Preview::Note(format!(
                "Too large to preview ({}). Save it to read it.",
                human_size(size)
            ));
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
    client::task(
        move || fetch_bytes(&client, &path),
        move |r| Msg::PreviewLoaded(id.clone(), r),
    )
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
    let Some(name) = state
        .files
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.name.clone())
    else {
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

/// Plays a video or opens a PDF: hands the file to the system's app from the open cache (see
/// `cache_copy`), so viewing never fills ~/Downloads. Download saves a copy there instead.
fn play(state: &State, id: &str, ctx: &Ctx) -> Task<Msg> {
    let Some((name, size)) = state
        .files
        .iter()
        .find(|d| d.id == id)
        .map(|d| (d.name.clone(), d.size))
    else {
        return Task::none();
    };
    let client = ctx.client.clone();
    let path = format!("/v1/files/{id}/content?download=1");
    let id = id.to_string();
    client::task(
        move || {
            let copy = cache_copy(&id, &name, size, || fetch_bytes(&client, &path))?;
            open_file(&copy)?;
            Ok(copy)
        },
        Msg::Played,
    )
}

/// Where a file opened for viewing is kept: a per-app folder under the system's temp directory, named
/// by the file's id and extension. The id is kept to plain characters, so it cannot name a path.
fn open_cache_path(id: &str, name: &str) -> PathBuf {
    let stem: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let ext: String = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let file = if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    };
    std::env::temp_dir().join("agentplaza-open").join(file)
}

/// The file's copy in the open cache, ready to hand to an app. A file's id names its content, so a copy
/// of the same size is reused without a download; otherwise `fetch` brings the bytes, which are written
/// under a `.part` name and renamed into place, so an interrupted write is never reused.
fn cache_copy(
    id: &str,
    name: &str,
    size: u64,
    fetch: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    let path = open_cache_path(id, name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == size) {
        return Ok(path.display().to_string());
    }
    let dir = path
        .parent()
        .ok_or_else(|| "the open cache has no folder".to_string())?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|e| e.to_string())?;
    let bytes = fetch()?;
    let part = path.with_extension("part");
    if let Err(e) = std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &path)) {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    Ok(path.display().to_string())
}

/// Opens a local file in the system's default app: `open` on macOS, the URL protocol handler on
/// Windows, `xdg-open` elsewhere. The path is one argument (no shell).
fn open_file(path: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn delete(id: String, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/files/{id}");
    client::task(move || client.delete(&path).map(|_| id), Msg::Deleted)
}

/// The agent tools, below the grid: the ones waiting for the owner's review, then the library's.
fn tools<'a>(state: &'a State, items: &mut Vec<Element<'a, Msg>>) {
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
    state.filter.is_none_or(|g| g == Group::of(&d.content_type))
        && (q.is_empty()
            || d.title.to_lowercase().contains(&q)
            || d.name.to_lowercase().contains(&q))
}

/// The files the list shows, in its order: the type filter and the search applied.
fn visible(state: &State) -> Vec<&Deliverable> {
    state.files.iter().filter(|d| matches(state, d)).collect()
}

/// Holds the keyboard's pick to the files the list shows now.
fn clamp_kbd(state: &mut State) {
    let len = visible(state).len();
    state.kbd = clamp(state.kbd, len);
}

/// A tile's tint, as the web's thumbnail colours: information, review, ok, warning, or the neutral one.
#[derive(Debug, Clone, Copy)]
enum Tint {
    Info,
    Review,
    Ok,
    Warn,
    Neutral,
}

impl Tint {
    /// The tint's ground and ink.
    fn colours(self) -> (Color, Color) {
        let p = crate::theme::palette();
        match self {
            Tint::Info => (p.info_soft, p.info),
            Tint::Review => (p.review_soft, p.review),
            Tint::Ok => (p.ok_soft, p.ok),
            Tint::Warn => (p.warn_soft, p.warn),
            Tint::Neutral => (p.sheet_2, p.ink_3),
        }
    }
}

/// The glyph and tint of a file's tile, by its extension (the web's `THUMB`); the rest get the neutral file tile.
fn glyph_for(name: &str, group: Group) -> (&'static str, Tint) {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "txt" => ("file-text", Tint::Info),
        "json" => ("braces", Tint::Review),
        "png" | "jpg" | "jpeg" | "svg" => ("image", Tint::Ok),
        "csv" => ("sheet", Tint::Warn),
        _ if group == Group::Video => ("play", Tint::Neutral),
        _ => ("file", Tint::Neutral),
    }
}

/// A tile's preview box: the tint's ground, its content centred, and an optional tag at the top right (the store).
fn thumb_box<'a>(tint: Tint, content: Element<'a, Msg>, tag: Option<&str>) -> Element<'a, Msg> {
    let (ground, _) = tint.colours();
    let body = container(content)
        .center(Fill)
        .height(Length::Fixed(THUMB_HEIGHT))
        .style(move |_: &Theme| container::Style {
            background: Some(ground.into()),
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        });
    match tag {
        Some(tag) => stack(vec![
            body.into(),
            container(ui::badge(tag, ui::Tone::Muted))
                .padding(8)
                .width(Fill)
                .align_x(Alignment::End)
                .into(),
        ])
        .into(),
        None => body.into(),
    }
}

/// The border and hover of a tile: the web's `.lib-tile`, its signal border when it is the open or the picked one.
fn tile_style(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_: &Theme, status: button::Status| {
        let p = crate::theme::palette();
        let skin = crate::theme::skin();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(p.raised.into()),
            text_color: p.ink,
            border: Border {
                color: if on { p.signal } else { p.rule },
                width: 1.0,
                radius: skin.radius_lg.into(),
            },
            shadow: if hovered {
                skin.shadow_card
            } else {
                Shadow::default()
            },
            snap: true,
        }
    }
}

/// The tiles of the files the tab and the filter leave, in the list's order; the open one and the arrow keys' pick
/// carry the signal border.
fn file_tiles<'a>(state: &'a State) -> Vec<Element<'a, Msg>> {
    visible(state)
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            tile(
                d,
                state.open.as_deref() == Some(d.id.as_str()),
                state.kbd == Some(i),
            )
        })
        .collect()
}

/// One file's tile, as the web's `.lib-tile`: its preview with the store tag, the title, the name, who and when, and
/// the size with its task and audience.
fn tile<'a>(d: &'a Deliverable, open: bool, picked: bool) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let title = if d.title.is_empty() {
        d.name.as_str()
    } else {
        d.title.as_str()
    };
    let (glyph, tint) = glyph_for(&d.name, Group::of(&d.content_type));
    let thumb = thumb_box(
        tint,
        column![
            ui::icon(glyph, 28.0, tint.colours().1),
            ui::mono(monogram(&d.name), 11.0, tint.colours().1),
        ]
        .spacing(6)
        .align_x(Alignment::Center)
        .into(),
        Some(d.store.as_str()),
    );
    let mut size_line = human_size(d.size);
    if let Some(task) = &d.task {
        size_line = format!("{size_line} · {task}");
    }
    if d.kind == "shared" && d.audience == "private" {
        size_line = format!("{size_line} · private · {} agents", d.allowed.len());
    }
    let body = column![
        thumb,
        text(title)
            .font(SEMIBOLD)
            .size(13.5)
            .color(p.ink)
            .wrapping(text::Wrapping::None),
        ui::mono(d.name.as_str(), 12.0, p.ink_3),
        ui::faint(format!("by {} · {}", d.author, ago(d.created_ms))),
        ui::faint(size_line),
    ]
    .spacing(8)
    .width(Fill);
    button(body)
        .on_press(Msg::Open(d.id.clone()))
        .style(tile_style(open || picked))
        .padding(10)
        .width(Fill)
        .into()
}

/// The artifacts' tiles, filtered by the search, as the web's artifact tiles.
fn artifact_tiles<'a>(state: &'a State) -> Vec<Element<'a, Msg>> {
    let q = state.query.trim().to_lowercase();
    state
        .artifacts
        .iter()
        .filter(|s| {
            q.is_empty()
                || s.title.to_lowercase().contains(&q)
                || s.kind.to_lowercase().contains(&q)
        })
        .map(artifact_tile)
        .collect()
}

/// One artifact's tile: its kind in the box, the title, the kind's chip, who and when, the version and Shared, and
/// the project. A press opens the artifact on its own screen.
fn artifact_tile<'a>(s: &'a apz_view::artifact::Summary) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let abbr: String = s.kind.chars().take(4).collect::<String>().to_uppercase();
    let mut body: Vec<Element<'a, Msg>> = vec![
        thumb_box(Tint::Neutral, ui::mono(abbr, 11.0, p.ink_2).into(), None),
        text(s.title.as_str())
            .font(SEMIBOLD)
            .size(13.5)
            .color(p.ink)
            .wrapping(text::Wrapping::None)
            .into(),
        ui::badge(s.kind.as_str(), ui::Tone::Muted),
        ui::faint(format!("by {} · {}", s.author, ago(s.updated_ms as i64))).into(),
        ui::faint(format!(
            "v{}{}",
            s.latest,
            if s.shared { " · Shared" } else { "" }
        ))
        .into(),
    ];
    if let Some(project) = &s.project {
        body.push(ui::mono(project.as_str(), 12.0, p.ink_3).into());
    }
    button(column(body).spacing(8).width(Fill))
        .on_press(Msg::Go(Id::Artifact(s.id.clone())))
        .style(tile_style(false))
        .padding(10)
        .width(Fill)
        .into()
}

/// The open file's panel, beside the grid as the web's `.lib-det`: its name and a close, then the preview, the
/// description, the facts and the actions (Share, Delete, Download; a delete asks first).
fn aside<'a>(state: &'a State, d: &'a Deliverable) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let title = if d.title.is_empty() {
        d.name.as_str()
    } else {
        d.title.as_str()
    };
    let head = container(
        row![
            text(title)
                .font(SEMIBOLD)
                .size(13.5)
                .color(p.ink)
                .width(Fill)
                .wrapping(text::Wrapping::None),
            ui::btn_sm("", Some("x"), ui::Kind::Ghost, Some(Msg::Close)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .width(Fill)
    .center_y(Length::Fixed(44.0));
    let mut body: Vec<Element<'a, Msg>> = vec![preview_block(state)];
    if let Some(desc) = &d.description {
        body.push(ui::faint(desc.as_str()).into());
    }
    body.push(facts(d));
    body.push(file_actions(d));
    container(column![
        head,
        ui::hairline(),
        container(column(body).spacing(12)).padding(16).width(Fill),
    ])
    .width(Fill)
    .style(ui::card_style)
    .into()
}

/// The open file's preview, by its kind (the state's `Preview`).
fn preview_block(state: &State) -> Element<'_, Msg> {
    let p = crate::theme::palette();
    let skin = crate::theme::skin();
    let id = state.open.clone().unwrap_or_default();
    match &state.preview {
        Preview::Idle => ui::faint("").into(),
        Preview::Loading => ui::loading(),
        Preview::Image(handle) => image(handle.clone())
            .width(Fill)
            .height(Length::Fixed(360.0))
            .into(),
        Preview::Text(s) => container(ui::scroll(ui::mono(s.as_str(), 12.0, p.ink_2)))
            .padding(12)
            .width(Fill)
            .max_height(320.0)
            .style(move |_: &Theme| container::Style {
                background: Some(p.sheet_2.into()),
                border: Border::default().rounded(skin.radius),
                ..Default::default()
            })
            .into(),
        Preview::Markdown(s) => ui::markdown(s),
        Preview::Note(s) => ui::faint(s.as_str()).into(),
        Preview::Video => column![
            container(ui::sans("▶", 40.0, p.ink_2))
                .padding(24)
                .width(Fill),
            ui::btn("Play", None, ui::Kind::Primary, Some(Msg::Play(id))),
        ]
        .spacing(12)
        .into(),
        Preview::Pdf => column![
            container(ui::sans("PDF", 40.0, p.ink_2))
                .padding(24)
                .width(Fill),
            ui::btn("Open", None, ui::Kind::Primary, Some(Msg::Play(id))),
        ]
        .spacing(12)
        .into(),
    }
}

/// The file's actions, right-aligned: Share and Delete (ghost), Download (primary). A delete asks first, in the
/// delete dialog (see `delete_dialog`).
fn file_actions<'a>(d: &'a Deliverable) -> Element<'a, Msg> {
    row![
        Space::new().width(Fill),
        ui::btn_sm(
            "Share",
            None,
            ui::Kind::Ghost,
            Some(Msg::ShareOpen(d.id.clone(), d.name.clone()))
        ),
        ui::btn_sm("Delete", None, ui::Kind::Ghost, Some(Msg::AskDelete)),
        ui::btn_sm(
            "Download",
            None,
            ui::Kind::Primary,
            Some(Msg::Save(d.id.clone()))
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn facts<'a>(d: &'a Deliverable) -> Element<'a, Msg> {
    let audience = if d.audience == "private" {
        format!("{} · {} agents", d.audience, d.allowed.len())
    } else {
        d.audience.clone()
    };
    let mut rows: Vec<Element<'a, Msg>> = vec![
        fact("Name", d.name.clone(), true),
        fact("Content type", d.content_type.clone(), true),
        fact(
            "Size",
            format!("{} ({} bytes)", human_size(d.size), d.size),
            false,
        ),
        fact("sha256", d.sha256.clone(), true),
        fact("Handed in by", d.author.clone(), false),
        fact("When", when(d.created_ms), false),
        fact("Store", d.store.clone(), false),
        fact("Audience", audience, false),
    ];
    if let Some(task) = &d.task {
        rows.push(fact("Task", task.clone(), true));
    }
    column(rows).spacing(6).into()
}

/// One fact: a faint label, then the value (mono for names, hashes and ids; serif otherwise).
fn fact<'a>(label: &'static str, value: String, mono: bool) -> Element<'a, Msg> {
    let value: Element<'a, Msg> = if mono {
        ui::sans(value, 12.0, crate::theme::palette().ink).into()
    } else {
        ui::prose(value).size(13.0).into()
    };
    row![
        container(ui::faint(label)).width(Length::Fixed(120.0)),
        value
    ]
    .spacing(8)
    .into()
}

// ---- Library tools --------------------------------------------------------------------------

fn tool_card<'a>(state: &'a State, t: &'a LibraryTool) -> Element<'a, Msg> {
    let me = ToolRef {
        name: t.name.clone(),
        version: t.version,
    };
    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!(
            "v{} · by {} · {} · {} calls",
            t.version,
            t.author,
            ago(t.created_ms),
            t.calls
        ))
        .into(),
    ];
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
        bar.push(ui::btn(
            "Publish",
            None,
            ui::Kind::Primary,
            Some(Msg::ToolAsk(ToolAction::Publish(me.clone()))),
        ));
        bar.push(ui::btn(
            "Reject",
            None,
            ui::Kind::Ghost,
            Some(Msg::ToolAsk(ToolAction::Reject(me.clone()))),
        ));
    }
    if !t.files.is_empty() {
        bar.push(ui::btn(
            "Files",
            None,
            ui::Kind::Ghost,
            Some(Msg::ToolFiles(me.clone())),
        ));
    }
    bar.push(ui::btn(
        "Remove",
        None,
        ui::Kind::Ghost,
        Some(Msg::ToolAsk(ToolAction::Remove(me.clone()))),
    ));
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
                ui::faint(format!(
                    "Removes v{} and its files from the library. This cannot be undone.",
                    t.version
                ))
                .into(),
            );
            body.push(confirm_row("Remove"));
        }
        _ => {}
    }

    if state.tool_open.as_ref() == Some(&me) {
        for f in &t.files {
            let label = format!("{} · {}", f.path, human_size(f.size));
            body.push(ui::btn(
                &label,
                None,
                ui::Kind::Ghost,
                Some(Msg::ToolFile(me.clone(), f.path.clone())),
            ));
        }
        if let Some(view) = state.tool_view.as_ref().filter(|v| v.tool == me) {
            body.push(ui::faint(view.path.as_str()).into());
            body.push(
                container(ui::sans(
                    view.content.as_str(),
                    12.0,
                    crate::theme::palette().ink,
                ))
                .padding(12)
                .width(Fill)
                .into(),
            );
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
            let content = value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(ToolFileView {
                tool,
                path,
                content,
            })
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
        ToolAction::Reject(tool) => review(
            client,
            tool,
            false,
            Some(note).filter(|n| !n.trim().is_empty()),
        ),
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
    client::task(
        move || client.post::<Value>(&path, &body).map(|_| label),
        Msg::Done,
    )
}

// ---- Helpers --------------------------------------------------------------------------------

/// The row an arrow key picks: the first row on a press with none picked, else one row down or up,
/// held to the first and the last. None when there are no rows.
fn step(cur: Option<usize>, len: usize, down: bool) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(match cur {
        None => 0,
        Some(i) if down => (i.min(last) + 1).min(last),
        Some(i) => i.min(last).saturating_sub(1),
    })
}

/// A pick held to a list of `len` rows: None once the list is empty, else at most its last row.
fn clamp(cur: Option<usize>, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    cur.map(|i| i.min(last))
}

fn is_pdf(content_type: &str) -> bool {
    content_type.eq_ignore_ascii_case("application/pdf")
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
    } else if d.name.to_lowercase().ends_with(".md")
        || d.content_type.eq_ignore_ascii_case("text/markdown")
    {
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

pub fn ago(ms: i64) -> String {
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
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn tool_path(tool: &ToolRef) -> String {
    format!("/v1/library/{}/{}", enc(&tool.name), tool.version)
}

/// Percent-encodes everything but unreserved characters (for path segments and query values).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The raw bytes of a hub path (the file's content), or the hub's error.
fn fetch_bytes(client: &client::Client, path: &str) -> Result<Vec<u8>, String> {
    let reply = apz_http::client::request(&client.target, "GET", path, None, client.timeout)
        .map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}", reply.status))
    }
}

/// Writes a download to ~/Downloads under the file's own name (never a path from the name). A download
/// never overwrites: when the name is taken, the first free one of `name (1).ext`, `name (2).ext`, …
/// is used.
pub fn save_download(name: &str, bytes: &[u8]) -> Result<String, String> {
    let dir = apz_os::dirs::home()
        .ok_or_else(|| "the home directory is not set".to_string())?
        .join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("deliverable");
    let mut n = 0;
    loop {
        let target = dir.join(numbered(file, n));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut out) => {
                if let Err(e) = out.write_all(bytes) {
                    // Do not leave a part-written file under the name the next download wants.
                    let _ = std::fs::remove_file(&target);
                    return Err(e.to_string());
                }
                return Ok(target.display().to_string());
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The name a download is tried under: the file's own name first (n = 0), then `name (n).ext`.
fn numbered(file: &str, n: u32) -> String {
    if n == 0 {
        return file.to_string();
    }
    let path = Path::new(file);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(file);
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem} ({n}).{ext}"),
        None => format!("{stem} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_copy, numbered, open_cache_path};
    use std::cell::Cell;

    #[test]
    fn the_open_cache_names_a_file_by_its_id_and_extension() {
        let path = open_cache_path("a1-b2", "Clip.MP4");
        assert_eq!(path.file_name().unwrap(), "a1-b2.MP4");
        assert_eq!(
            path.parent().unwrap().file_name().unwrap(),
            "agentplaza-open"
        );
        // A hostile id keeps only plain characters, so it stays one file name in that folder.
        assert_eq!(
            open_cache_path("../x/y", "notes").file_name().unwrap(),
            "xy"
        );
    }

    #[test]
    fn an_opened_file_is_fetched_once_and_then_reused_at_the_same_size() {
        let id = format!(
            "test-reuse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let fetches = Cell::new(0);
        let bytes = b"%PDF-1.4 test".to_vec();
        let fetch = || {
            fetches.set(fetches.get() + 1);
            Ok(bytes.clone())
        };
        let first = cache_copy(&id, "doc.pdf", bytes.len() as u64, fetch).unwrap();
        let again = cache_copy(&id, "doc.pdf", bytes.len() as u64, || {
            fetches.set(fetches.get() + 1);
            Ok(Vec::new())
        })
        .unwrap();
        assert_eq!(first, again);
        assert_eq!(fetches.get(), 1);
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
        let _ = std::fs::remove_file(&first);
    }

    #[test]
    fn a_taken_download_name_gets_a_number_before_the_extension() {
        assert_eq!(numbered("report.pdf", 0), "report.pdf");
        assert_eq!(numbered("report.pdf", 1), "report (1).pdf");
        assert_eq!(numbered("report.pdf", 2), "report (2).pdf");
        assert_eq!(numbered("notes", 1), "notes (1)");
        assert_eq!(numbered("archive.tar.gz", 1), "archive.tar (1).gz");
    }
}
