//! The shell around every screen, as the Loom and Graphite consoles draw it (the web's `ui/shell.ts`): the sidebar
//! (the brand, search, the screens and More, what is moving inside each project, the foot with the hub and STOP ALL),
//! the main column's head (the scope pill, the crumbs, the live line), Settings with its own index, the search
//! palette, the held banner and the New project dialog.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use iced::widget::{
    Space, Stack, button, column, container, mouse_area, row, stack, text, text_input,
};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme, alignment,
    event, mouse,
};

use super::events;
use super::screens::{Id, NAV, appearance, projects::Tab};
use super::{Ctx, new_project, roles, scope, toast, ui};
use crate::theme::SANS;
use apz_theme::appearance::ModeChoice;

/// The open sidebar's default width (the web's `DEFAULT_WIDTH`) and its limits.
const DEFAULT_WIDTH: f32 = 248.0;
const MIN_WIDTH: f32 = 200.0;
const MAX_WIDTH: f32 = 420.0;
/// The resize handle's width at the sidebar's right edge; its signal line is 2 px of it.
const HANDLE_W: f32 = 6.0;

/// The sidebar mode the last run left ("open" or "rail"), Open when none was saved. Tests never read the owner's
/// settings file.
fn saved_mode() -> Mode {
    if cfg!(test) {
        return Mode::Open;
    }
    match super::boot::saved_setting("sidebar")
        .as_ref()
        .and_then(|v| v.as_str())
    {
        Some("rail") => Mode::Rail,
        _ => Mode::Open,
    }
}

/// Saves the sidebar mode for the next run (not from tests).
fn save_mode(name: &str) -> std::io::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    super::boot::save_setting("sidebar", serde_json::json!(name))
}

/// The sidebar's display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Open,
    Rail,
}

#[derive(Debug)]
pub struct State {
    pub current: Id,
    pub mode: Mode,
    /// The open sidebar's width (200–420).
    pub width: f32,
    /// The plaza's held state (STOP ALL), as the hub reports it.
    pub paused: bool,
    pub held_note: String,
    /// What the hold stopped and who held it: `cancelled`, `since_ms` and `by` from `/v1/plaza/state`.
    pub held_cancelled: u64,
    pub held_since_ms: u64,
    pub held_by: String,
    /// Counts per screen key (agents, approvals, …).
    pub counts: BTreeMap<&'static str, u32>,
    /// The top bar's working and idle agent counts (`/v1/overview`).
    pub working: u32,
    pub idle: u32,
    /// The top bar's node indicator (`noded` in `/v1/overview`).
    pub node_ok: bool,
    pub hub_ok: bool,
    /// The hub's live event stream is connected: the top bar's "Live", else "Reconnecting…".
    pub live: bool,
    /// The hub's address, shown faint in the sidebar footer.
    pub hub: String,
    /// The graphics adapter, when it is a software fallback (the 3D floor will crawl on it).
    pub software_gpu: Option<String>,
    /// The hub's target, once the shell has one: the top bar's frame stream runs on it.
    pub target: Option<apz_http::client::Target>,
    /// A count reload is in flight; a frame that asks for one meanwhile sets `again`.
    pub loading: bool,
    pub again: bool,
    /// The toasts on screen, newest last; they outlive the screen that raised them.
    pub toasts: Vec<toast::Toast>,
    /// An open confirmation (STOP ALL or RESUME), asked before the hub is called.
    pub confirm: Option<Confirm>,
    /// A resize drag in progress: the pointer's window x and the width when it began.
    drag: Option<(f32, f32)>,
    /// The pointer's window x as last seen over the handle: where a drag starts from.
    pointer_x: f32,
    /// The pointer is over the handle (its signal line shows, as it does while dragging).
    handle_hot: bool,
    /// The entry the tour is describing: drawn marked, as on the web.
    pub highlight: Option<Id>,
    /// The project the console is scoped to (the console's `Ctx::project`, kept here for the top bar).
    pub project: Option<String>,
    /// Every project, as `/v1/projects` lists them: the switcher offers the live ones.
    pub projects: Vec<serde_json::Value>,
    /// The New project dialog, while it is open.
    pub new_project: Option<new_project::Form>,
    /// More is open in the sidebar (the screens past the first six).
    pub more: bool,
    /// The sidebar's lists: approvals waiting, tasks, the owner's conversations, and the agents (for their labels and the
    /// live line), from `/v1/approvals`, `/v1/tasks`, `/v1/owner/conversations` and `/v1/agents`.
    pub approvals: Vec<serde_json::Value>,
    pub tasks: Vec<serde_json::Value>,
    pub conversations: Vec<serde_json::Value>,
    pub agents: Vec<serde_json::Value>,
    /// The projects whose rows are folded in the sidebar.
    pub folded: std::collections::BTreeSet<String>,
    /// The search palette, while it is open: the query and the selected row.
    pub palette: Option<(String, usize)>,
    /// The scope menu or the owner's menu, while one is open.
    pub menu: Option<Menu>,
    /// Which live line the head shows (it turns every few seconds).
    pub live_at: usize,
}

/// A menu over the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Scope,
    Owner,
}

impl Default for State {
    fn default() -> Self {
        State {
            current: Id::Overview,
            mode: saved_mode(),
            width: DEFAULT_WIDTH,
            paused: false,
            held_note: String::new(),
            held_cancelled: 0,
            held_since_ms: 0,
            held_by: String::new(),
            counts: Default::default(),
            working: 0,
            idle: 0,
            node_ok: false,
            hub_ok: false,
            live: false,
            hub: String::new(),
            software_gpu: None,
            target: None,
            loading: false,
            again: false,
            toasts: Vec::new(),
            confirm: None,
            drag: None,
            pointer_x: 0.0,
            handle_hot: false,
            highlight: None,
            project: None,
            projects: Vec::new(),
            new_project: None,
            more: false,
            approvals: Vec::new(),
            tasks: Vec::new(),
            conversations: Vec::new(),
            agents: Vec::new(),
            folded: Default::default(),
            palette: None,
            menu: None,
            live_at: 0,
        }
    }
}

/// One entry of the top bar's project switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeChoice {
    All,
    /// A project: its id and title.
    Project(String, String),
    New,
}

impl std::fmt::Display for ScopeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeChoice::All => f.write_str("All projects"),
            ScopeChoice::Project(_, title) => f.write_str(title),
            ScopeChoice::New => f.write_str("New project…"),
        }
    }
}

/// The switcher's entries: All projects, each live project newest first, and New project…. A scope on a project that
/// is over (or not listed yet) still shows, so the switcher says where the console is.
pub fn scope_choices(projects: &[serde_json::Value], current: Option<&str>) -> Vec<ScopeChoice> {
    let mut choices = vec![ScopeChoice::All];
    for p in scope::live_projects(projects) {
        if let Some(id) = p["id"].as_str() {
            choices.push(ScopeChoice::Project(
                id.to_string(),
                scope::title_in(projects, id),
            ));
        }
    }
    if let Some(id) = current
        && !choices
            .iter()
            .any(|c| matches!(c, ScopeChoice::Project(pid, _) if pid == id))
    {
        choices.push(ScopeChoice::Project(
            id.to_string(),
            scope::title_in(projects, id),
        ));
    }
    choices.push(ScopeChoice::New);
    choices
}

/// The screens that hold the plaza's settings, which a project overrides in its own (the web's `PLAZA_SETTINGS`):
/// under a project, they say so.
fn plaza_settings(id: &Id) -> bool {
    matches!(
        id,
        Id::Models
            | Id::Tools
            | Id::Mcp
            | Id::Prompts
            | Id::Skills
            | Id::Rules
            | Id::Vault
            | Id::Egress
    )
}

/// An open confirmation: STOP ALL with its optional reason, or RESUME.
#[derive(Debug, Clone)]
pub enum Confirm {
    Stop(String),
    Resume,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    /// Opens the IDE on the workspace it last had (or its picker): the top bar's button and Ctrl+Shift+E.
    OpenIde,
    ToggleSidebar,
    /// Width change in px (negative narrows); clamped to 200–420.
    Resize(f32),
    /// The pointer over the resize handle, at `x` px from the handle's left edge.
    HandleMove(f32),
    HandleEnter,
    HandleExit,
    /// A press on the handle starts a drag.
    DragStart,
    /// The pointer is at `x` px in the window while dragging.
    DragMove(f32),
    /// The button was let go: the drag ends.
    DragEnd,
    /// A double-click on the handle: back to the default width.
    ResetWidth,
    /// Opens the STOP ALL confirmation.
    StopAll,
    /// Opens the RESUME confirmation.
    Resume,
    /// The STOP ALL dialog's reason field (capped at 200 characters).
    StopReason(String),
    /// The confirmations' action buttons, and Cancel.
    StopGo,
    ResumeGo,
    CancelConfirm,
    /// `GET /v1/overview` + `GET /v1/plaza/state` answered.
    Loaded(Result<serde_json::Value, String>),
    /// `POST /v1/plaza/stop` or `/v1/plaza/resume` answered.
    Done(Result<serde_json::Value, String>),
    /// A hub frame from the top bar's stream.
    Frame(events::Event),
    /// The events socket came up (true) or went down (false), from the top bar's link stream.
    Live(bool),
    /// Puts a toast up for `toast::LIFETIME`.
    Toast(toast::Kind, String),
    /// The toasts' clock: takes down the ones whose time is up. Ticks only while a toast is on screen.
    ToastTick(Instant),
    /// The top bar's light/dark switch.
    ToggleTheme,
    /// The OS appearance, at start and when it changes; it counts only until the owner chooses a theme.
    Appearance(iced::theme::Mode),
    /// The hub's answer to a light or dark change from the top bar: its document, or why it was not saved.
    ModeSaved(Result<apz_theme::appearance::Document, String>),
    /// Scopes the console to a project (None: All projects) and opens a screen (None: the one on show, or its
    /// list). The console handles it: the scope is its `Ctx`.
    Scope(Option<String>, Option<Id>),
    /// Opens the New project dialog (the switcher's last entry).
    OpenNewProject,
    NewProject(new_project::Msg),
    /// More or Less in the sidebar.
    ToggleMore,
    /// Folds or unfolds a project's rows in the sidebar.
    Fold(String),
    /// Ctrl+K and the sidebar's search: the palette opens; its query, a move of the selection, a pick, Escape.
    OpenPalette,
    PaletteQuery(String),
    PaletteMove(i32),
    PalettePick,
    PaletteGo(Id),
    ClosePalette,
    /// The scope pill's and the owner's menus.
    OpenMenu(Menu),
    CloseMenu,
    /// The live line turns to its next item.
    LiveTick,
}

/// The shell's own data: the plaza's held state, the sidebar counts and the projects the switcher offers.
pub fn load(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    super::client::task(
        move || {
            let overview: serde_json::Value = client.get("/v1/overview")?;
            let control: serde_json::Value = client.get("/v1/plaza/state")?;
            // The projects are the switcher's; a hub that cannot list them leaves the last list as it was.
            let projects = client
                .get::<serde_json::Value>("/v1/projects")
                .unwrap_or_default();
            // The sidebar's lists: a hub that cannot answer one leaves that list as it was.
            let approvals = client
                .get::<serde_json::Value>("/v1/approvals")
                .unwrap_or_default();
            let tasks = client
                .get::<serde_json::Value>("/v1/tasks")
                .unwrap_or_default();
            let conversations = client
                .get::<serde_json::Value>("/v1/owner/conversations")
                .unwrap_or_default();
            let agents = client
                .get::<serde_json::Value>("/v1/agents")
                .unwrap_or_default();
            Ok(serde_json::json!({
                "overview": overview, "control": control, "projects": projects, "approvals": approvals,
                "tasks": tasks, "conversations": conversations, "agents": agents,
            }))
        },
        Msg::Loaded,
    )
}

fn post(ctx: &Ctx, path: &'static str, body: serde_json::Value) -> Task<Msg> {
    let client = ctx.client.clone();
    super::client::task(
        move || client.post::<serde_json::Value>(path, &body),
        Msg::Done,
    )
}

/// Returns a task and, when the user picked a screen, where to go.
pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> (Task<Msg>, Option<Id>) {
    match msg {
        Msg::OpenIde => {
            let id = Id::Ide(super::screens::ide::remembered());
            state.current = id.clone();
            (Task::none(), Some(id))
        }
        Msg::Go(id) => {
            state.menu = None;
            state.current = id.clone();
            (Task::none(), Some(id))
        }
        // The console scopes itself (it owns the `Ctx`); the message never reaches here from it.
        Msg::Scope(..) => (Task::none(), None),
        Msg::OpenNewProject => {
            state.menu = None;
            state.new_project = Some(new_project::Form::default());
            (Task::none(), None)
        }
        Msg::ToggleMore => {
            state.more = !state.more;
            (Task::none(), None)
        }
        Msg::Fold(id) => {
            if !state.folded.remove(&id) {
                state.folded.insert(id);
            }
            (Task::none(), None)
        }
        Msg::OpenPalette => {
            state.menu = None;
            state.palette = Some((String::new(), 0));
            (iced::widget::operation::focus(PALETTE_INPUT.clone()), None)
        }
        Msg::PaletteQuery(q) => {
            state.palette = Some((q, 0));
            (Task::none(), None)
        }
        Msg::PaletteMove(d) => {
            if let Some((q, sel)) = &mut state.palette {
                let n = palette_items(state_lists(&state.projects, &state.agents), q)
                    .len()
                    .max(1) as i32;
                *sel = (((*sel as i32) + d).rem_euclid(n)) as usize;
            }
            (Task::none(), None)
        }
        Msg::PalettePick => {
            let to = state.palette.as_ref().and_then(|(q, sel)| {
                palette_items(state_lists(&state.projects, &state.agents), q)
                    .get(*sel)
                    .map(|it| it.to.clone())
            });
            state.palette = None;
            match to {
                Some(id) => {
                    state.current = id.clone();
                    (Task::none(), Some(id))
                }
                None => (Task::none(), None),
            }
        }
        Msg::PaletteGo(id) => {
            state.palette = None;
            state.current = id.clone();
            (Task::none(), Some(id))
        }
        Msg::ClosePalette => {
            state.palette = None;
            (Task::none(), None)
        }
        Msg::OpenMenu(m) => {
            state.menu = if state.menu == Some(m) { None } else { Some(m) };
            (Task::none(), None)
        }
        Msg::CloseMenu => {
            state.menu = None;
            (Task::none(), None)
        }
        Msg::LiveTick => {
            state.live_at = state.live_at.wrapping_add(1);
            (Task::none(), None)
        }
        Msg::NewProject(msg) => {
            let Some(form) = state.new_project.as_mut() else {
                return (Task::none(), None);
            };
            let (task, effect) = new_project::update(form, msg, ctx);
            match effect {
                new_project::Effect::None => (task.map(Msg::NewProject), None),
                new_project::Effect::Close => {
                    state.new_project = None;
                    (Task::none(), None)
                }
                // The conversation opens in the IDE, where the orchestrator drafts the project beside it.
                new_project::Effect::Conversation(id) => {
                    state.new_project = None;
                    let id = Id::Ide(Some(id));
                    state.current = id.clone();
                    (Task::none(), Some(id))
                }
                // A project opened directly: the console is scoped to it, on its page.
                new_project::Effect::Project(id) => {
                    state.new_project = None;
                    (
                        Task::done(Msg::Scope(Some(id.clone()), Some(Id::Project(id)))),
                        None,
                    )
                }
            }
        }
        Msg::ToggleTheme => {
            let mode = if crate::theme::is_light() {
                ModeChoice::Dark
            } else {
                ModeChoice::Light
            };
            (appearance::set_mode(ctx, mode, Msg::ModeSaved), None)
        }
        Msg::ModeSaved(res) => match appearance::mode_saved(res) {
            Ok(()) => (Task::none(), None),
            Err(e) => (Task::done(Msg::Toast(toast::Kind::Error, e)), None),
        },
        Msg::Appearance(mode) => {
            crate::theme::follow_system(mode);
            (Task::none(), None)
        }
        Msg::ToggleSidebar => {
            state.mode = match state.mode {
                Mode::Open => Mode::Rail,
                Mode::Rail => Mode::Open,
            };
            // Remembered across runs, as the web keeps it in localStorage (`apz.sidebar`).
            let name = if state.mode == Mode::Rail {
                "rail"
            } else {
                "open"
            };
            if let Err(err) = save_mode(name) {
                eprintln!("agentplaza-desktop: the sidebar mode was not saved: {err}");
            }
            (Task::none(), None)
        }
        Msg::Resize(delta) => {
            state.width = (state.width + delta).clamp(MIN_WIDTH, MAX_WIDTH);
            (Task::none(), None)
        }
        Msg::HandleMove(x) => {
            // The handle sits at the sidebar's right edge, so its left edge is `width - HANDLE_W`.
            state.pointer_x = state.width - HANDLE_W + x;
            (Task::none(), None)
        }
        Msg::HandleEnter => {
            state.handle_hot = true;
            (Task::none(), None)
        }
        Msg::HandleExit => {
            state.handle_hot = false;
            (Task::none(), None)
        }
        Msg::DragStart => {
            state.drag = Some((state.pointer_x, state.width));
            (Task::none(), None)
        }
        Msg::DragMove(x) => {
            if let Some((x0, w0)) = state.drag {
                state.width = (w0 + x - x0).clamp(MIN_WIDTH, MAX_WIDTH);
            }
            (Task::none(), None)
        }
        Msg::DragEnd => {
            state.drag = None;
            (Task::none(), None)
        }
        Msg::ResetWidth => {
            state.drag = None;
            state.width = DEFAULT_WIDTH;
            (Task::none(), None)
        }
        Msg::StopAll => {
            state.confirm = Some(Confirm::Stop(String::new()));
            (Task::none(), None)
        }
        Msg::Resume => {
            state.confirm = Some(Confirm::Resume);
            (Task::none(), None)
        }
        Msg::StopReason(reason) => {
            if let Some(Confirm::Stop(r)) = &mut state.confirm {
                *r = reason.chars().take(200).collect();
            }
            (Task::none(), None)
        }
        Msg::StopGo => {
            let reason = match &state.confirm {
                Some(Confirm::Stop(r)) => r.trim().to_string(),
                _ => String::new(),
            };
            state.confirm = None;
            (
                post(
                    ctx,
                    "/v1/plaza/stop",
                    serde_json::json!({ "reason": reason }),
                ),
                None,
            )
        }
        Msg::ResumeGo => {
            state.confirm = None;
            (post(ctx, "/v1/plaza/resume", serde_json::json!({})), None)
        }
        Msg::CancelConfirm => {
            state.confirm = None;
            (Task::none(), None)
        }
        Msg::Done(Ok(_)) => (load(ctx), None),
        Msg::Done(Err(_)) => (Task::none(), None),
        Msg::Loaded(Ok(v)) => {
            state.hub_ok = v["overview"]["hub"].as_bool().unwrap_or(false);
            state.node_ok = v["overview"]["noded"].as_bool().unwrap_or(false);
            state.working = v["overview"]["agents"]["working"].as_u64().unwrap_or(0) as u32;
            state.idle = v["overview"]["agents"]["idle"].as_u64().unwrap_or(0) as u32;
            state.counts.insert(
                "agents",
                v["overview"]["agents"]["total"].as_u64().unwrap_or(0) as u32,
            );
            set_control(state, &v["control"]);
            state.hub = ctx.client.base().to_string();
            state.target = Some(ctx.client.target.clone());
            state.loading = false;
            let again = if std::mem::take(&mut state.again) {
                load(ctx)
            } else {
                Task::none()
            };
            if let Some(list) = v["approvals"].as_array() {
                state.approvals = list
                    .iter()
                    .filter(|a| a["status"].as_str().is_none_or(|s| s == "pending"))
                    .cloned()
                    .collect();
                state
                    .counts
                    .insert("approvals", state.approvals.len() as u32);
            }
            if let Some(list) = v["tasks"]["tasks"].as_array().or(v["tasks"].as_array()) {
                state.tasks = list.clone();
            }
            if let Some(list) = v["conversations"].as_array() {
                state.conversations = list.clone();
            }
            if let Some(list) = v["agents"].as_array() {
                state.agents = list.clone();
            }
            if let Some(list) = v["projects"].as_array() {
                state.projects = list.clone();
                scope::remember_titles(list);
                // A remembered project that no longer exists sends the console back to All projects.
                if let Some(current) = &state.project
                    && !list.iter().any(|p| p["id"].as_str() == Some(current))
                {
                    return (
                        Task::batch([again, Task::done(Msg::Scope(None, None))]),
                        None,
                    );
                }
            }
            (again, None)
        }
        Msg::Loaded(Err(_)) => {
            state.hub_ok = false;
            state.node_ok = false;
            state.loading = false;
            (
                if std::mem::take(&mut state.again) {
                    load(ctx)
                } else {
                    Task::none()
                },
                None,
            )
        }
        Msg::Toast(kind, text) => {
            toast::push(&mut state.toasts, kind, text, Instant::now());
            (Task::none(), None)
        }
        Msg::ToastTick(now) => {
            toast::expire(&mut state.toasts, now);
            (Task::none(), None)
        }
        Msg::Live(up) => {
            state.live = up;
            (Task::none(), None)
        }
        Msg::Frame(event) => {
            if apply_frame(state, &event.kind, &event.data) {
                if state.loading {
                    state.again = true;
                } else {
                    state.loading = true;
                    return (load(ctx), None);
                }
            }
            (Task::none(), None)
        }
    }
}

/// The hold's fields from `/v1/plaza/state` or a `plaza` frame (the same shape).
fn set_control(state: &mut State, v: &serde_json::Value) {
    state.paused = v["paused"].as_bool().unwrap_or(false);
    state.held_note = v["reason"].as_str().unwrap_or("").to_string();
    state.held_cancelled = v["cancelled"].as_u64().unwrap_or(0);
    state.held_since_ms = v["since_ms"].as_u64().unwrap_or(0);
    state.held_by = v["by"].as_str().unwrap_or("").to_string();
}

/// Applies one hub frame to the top bar: a `plaza` frame sets the hold as it is; an agent or work change
/// moves the counts, so it asks for a reload of `/v1/overview`. Returns true when that reload is wanted.
fn apply_frame(state: &mut State, kind: &str, data: &serde_json::Value) -> bool {
    match kind {
        "plaza" => {
            set_control(state, data);
            false
        }
        "agent" | "work" => true,
        _ => false,
    }
}

/// Whether `current` is drawn over the whole window, without the console's sidebar, top bar or banners.
/// The IDE is a view of its own: it draws its own header, and the console's chrome stays off it.
pub fn full_window(current: &Id) -> bool {
    matches!(current, Id::Ide(_))
}

/// The shell around `screen`; the shell's own messages are lifted into the app's with `wrap`.
pub fn view<'a, M: Clone + 'a>(
    state: &'a State,
    screen: Element<'a, M>,
    self_scrolling: bool,
    wrap: fn(Msg) -> M,
) -> Element<'a, M> {
    let base: Element<'a, M> = if full_window(&state.current) {
        container(screen)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(ui::paper)
            .into()
    } else {
        let mut main = column![top_bar(state, wrap)];
        if state.paused {
            main = main.push(held_banner(state, wrap));
        }
        if let Some(project) = &state.project
            && plaza_settings(&state.current)
        {
            main = main.push(settings_note(project, wrap));
        }
        if let Some(adapter) = &state.software_gpu {
            main = main.push(gpu_banner(adapter));
        }
        // A screen that does not scroll its own page is wrapped in one, so a squeezed screen scrolls instead of clipping.
        let screen = if self_scrolling || manages_scroll(&state.current) {
            screen
        } else {
            ui::scroll(screen)
                .id(SCREEN_SCROLL.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        // Settings: its index beside the screen (the web's `.settings` with `.snav`).
        let body: Element<'a, M> = match settings_group(&state.current) {
            Some(_) => row![
                settings_index(&state.current, wrap),
                container(screen).width(Length::Fill).height(Length::Fill)
            ]
            .height(Length::Fill)
            .into(),
            None => screen,
        };
        main = main.push(container(body).width(Length::Fill).height(Length::Fill));
        let main = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(ui::paper);
        row![sidebar(state, wrap), main].height(Length::Fill).into()
    };
    let mut layers = Stack::new().push(base);
    if let Some(m) = state.menu {
        layers = layers.push(menu_view(state, m, wrap));
    }
    if let Some((q, sel)) = &state.palette {
        layers = layers.push(palette_view(state, q, *sel, wrap));
    }
    if let Some(which) = &state.confirm {
        layers = layers.push(confirm_view(which, wrap));
    }
    if let Some(form) = &state.new_project {
        layers = layers.push(new_project::view(form).map(move |m| wrap(Msg::NewProject(m))));
    }
    if !state.toasts.is_empty() {
        layers = layers.push(toast::view(&state.toasts));
    }
    layers.into()
}

/// The id of the wrap around a screen that does not scroll its own page; the Machines screen scrolls
/// to a machine's section through it.
pub static SCREEN_SCROLL: std::sync::LazyLock<iced::widget::Id> =
    std::sync::LazyLock::new(iced::widget::Id::unique);

/// The id of the sidebar's list of entries: the tour scrolls it to the entry it describes.
pub static SIDEBAR_SCROLL: std::sync::LazyLock<iced::widget::Id> =
    std::sync::LazyLock::new(iced::widget::Id::unique);

/// Whether the sidebar entry `entry` is the screen that shows `current`: a task opened from a project is
/// shown by the Tasks screen.
fn shows(entry: &Id, current: &Id) -> bool {
    entry == current
        || matches!(
            (entry, current),
            (Id::Tasks, Id::Task(_))
                | (Id::Projects, Id::Project(_))
                | (Id::Projects, Id::ProjectTab(..))
                | (Id::Work, Id::WorkOn(_))
                | (Id::Library, Id::Artifact(_))
        )
}

/// Screens whose view scrolls its own page (a `scrollable` inside `ui::sheet`), or that handle their own
/// pointer input (a 3D view's canvas). The shell wraps every other screen; add a screen here once its view scrolls itself.
fn manages_scroll(id: &super::screens::Id) -> bool {
    use super::screens::Id;
    matches!(
        id,
        Id::Overview
            | Id::Metrics
            | Id::Approvals
            | Id::Agents
            | Id::Projects
            | Id::Project(_)
            | Id::ProjectTab(..)
            | Id::Tasks
            | Id::Task(_)
            | Id::Library
            | Id::Ide(_)
            | Id::Board
            | Id::Treasury
            | Id::Rules
            | Id::Tools
            | Id::Mcp
            | Id::Agent(_)
    )
}

pub fn subscription(state: &State) -> Subscription<Msg> {
    let frames = match &state.target {
        Some(target) => events::top_bar(target).map(Msg::Frame),
        None => Subscription::none(),
    };
    let live = match &state.target {
        Some(target) => events::link(target).map(Msg::Live),
        None => Subscription::none(),
    };
    // The toasts' clock runs only while a toast is on screen (a quarter-second step ends them within 4.25 s).
    let clock = if state.toasts.is_empty() {
        Subscription::none()
    } else {
        iced::time::every(Duration::from_millis(250)).map(Msg::ToastTick)
    };
    // The `[` key cycles the sidebar, as on the web. A key a focused text field takes is captured
    // (status Captured), so it does not reach here while the STOP ALL reason is being typed.
    let keys = iced::event::listen_with(|event, status, _| match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Character(c),
            modifiers,
            ..
        }) if status == iced::event::Status::Ignored
            && c.as_str() == "["
            && !modifiers.control()
            && !modifiers.alt()
            && !modifiers.logo() =>
        {
            Some(Msg::ToggleSidebar)
        }
        // Ctrl+Shift+E (Cmd+Shift+E on macOS) opens the IDE; a focused text field that takes the key keeps it.
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. })
            if super::screens::ide::open_shortcut(
                &key,
                modifiers,
                status == iced::event::Status::Captured,
            ) =>
        {
            Some(Msg::OpenIde)
        }
        // Ctrl+K (Cmd+K on macOS) opens the search palette, as on the web.
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Character(c),
            modifiers,
            ..
        }) if c.as_str() == "k"
            && modifiers.command()
            && !modifiers.shift()
            && !modifiers.alt() =>
        {
            Some(Msg::OpenPalette)
        }
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(named),
            ..
        }) => match named {
            iced::keyboard::key::Named::ArrowDown => Some(Msg::PaletteMove(1)),
            iced::keyboard::key::Named::ArrowUp => Some(Msg::PaletteMove(-1)),
            iced::keyboard::key::Named::Escape => Some(Msg::ClosePalette),
            _ => None,
        },
        _ => None,
    });
    // The live line in the head turns every 4.2 s while agents work.
    let live_tick = if state.agents.iter().any(|a| a["status"] == "working") {
        iced::time::every(Duration::from_millis(4200)).map(|_| Msg::LiveTick)
    } else {
        Subscription::none()
    };
    // While a resize drag runs, the pointer's moves and the button's release come from anywhere in
    // the window (the pointer can leave the handle), so they are listened to for the drag only.
    let drag = if state.drag.is_some() {
        event::listen_with(drag_event)
    } else {
        Subscription::none()
    };
    Subscription::batch([frames, live, clock, keys, drag, live_tick])
}

fn drag_event(event: iced::Event, _: event::Status, _: iced::window::Id) -> Option<Msg> {
    match event {
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Msg::DragMove(position.x))
        }
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Msg::DragEnd),
        _ => None,
    }
}

/// The confirmation over the shell, worded as the web's: STOP ALL asks for an optional reason, RESUME asks once.
fn confirm_view<'a, M: Clone + 'a>(which: &'a Confirm, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let p = crate::theme::palette();
    let close = wrap(Msg::CancelConfirm);
    let cancel = || {
        ui::btn(
            "Cancel",
            None,
            ui::Kind::Ghost,
            Some(wrap(Msg::CancelConfirm)),
        )
    };
    let (title, body, go): (&str, Element<'a, M>, Element<'a, M>) = match which {
        Confirm::Stop(reason) => (
            "Stop all agents?",
            column![
                ui::sans(
                    "Every agent's running work is cancelled and nothing new starts until you resume. Queued work, VMs and workspaces are kept.",
                    13.0,
                    p.ink_2
                ),
                ui::label("Reason"),
                text_input("Why (optional)", reason)
                    .on_input(move |s| wrap(Msg::StopReason(s)))
                    .padding(8),
                ui::faint("Optional. Shown on the held banner."),
            ]
            .spacing(10)
            .into(),
            ui::btn("Stop all", Some("octagon-pause"), ui::Kind::Danger, Some(wrap(Msg::StopGo))),
        ),
        Confirm::Resume => (
            "Resume the plaza?",
            ui::sans(
                "New work can start again. The items cancelled by the stop stay cancelled.",
                13.0,
                p.ink_2,
            )
            .into(),
            ui::btn("Resume", Some("play"), ui::Kind::Primary, Some(wrap(Msg::ResumeGo))),
        ),
    };
    ui::modal(title, body, vec![cancel(), go], close, 440.0)
}

/// The sidebar's screens (the web's `NAV` in `ui/shell.ts`): the first six, then More.
const MAIN: &[(Id, &str, &str)] = &[
    (Id::Overview, "Overview", "house"),
    (Id::Approvals, "Approvals", "shield-check"),
    (Id::Work, "Work", "workflow"),
    (Id::History, "Weft", "git-branch"),
    (Id::Agents, "Agents", "bot"),
    (Id::Machines, "Machines", "server"),
];
const MORE: &[(Id, &str, &str)] = &[
    (Id::Tasks, "Tasks", "list-checks"),
    (Id::Metrics, "Metrics", "chart-column"),
    (Id::Library, "Library", "library"),
    (Id::Board, "Board", "pin"),
    (Id::Treasury, "Treasury", "coins"),
    (Id::Rules, "Rules", "scale"),
    (Id::Lab, "Lab", "flask-conical"),
];

/// Settings: its groups and their screens (the web's `SETTINGS`), each group with its icon.
pub const SETTINGS: &[(&str, &str, &str, &[(Id, &str)])] = &[
    ("models", "Models", "cpu", &[(Id::Models, "Models")]),
    (
        "appearance",
        "Appearance",
        "palette",
        &[(Id::Appearance, "Appearance")],
    ),
    (
        "agents",
        "Agents",
        "bot",
        &[(Id::Prompts, "Prompts"), (Id::Skills, "Skills")],
    ),
    (
        "connections",
        "Connections",
        "plug",
        &[
            (Id::Tools, "Tools"),
            (Id::Mcp, "MCP"),
            (Id::Platforms, "Git platforms"),
        ],
    ),
    (
        "secrets",
        "Secrets",
        "key-round",
        &[(Id::Vault, "Vault"), (Id::Egress, "Egress credentials")],
    ),
    (
        "compute",
        "Compute",
        "server",
        &[
            (Id::Nodes, "Nodes"),
            (Id::Workstations, "Workstations"),
            (Id::Volumes, "Volumes"),
            (Id::E2b, "E2B API"),
        ],
    ),
    ("health", "Health", "heart-pulse", &[(Id::Setup, "Setup")]),
];

/// The settings group a screen sits in, or None when it is not a settings screen.
pub fn settings_group(
    id: &Id,
) -> Option<&'static (
    &'static str,
    &'static str,
    &'static str,
    &'static [(Id, &'static str)],
)> {
    let id = match id {
        Id::ThemeEditor(_) => &Id::Appearance,
        other => other,
    };
    SETTINGS.iter().find(|g| g.3.iter().any(|(k, _)| k == id))
}

/// The project stages as the sidebar names them.
fn stage_label(stage: &str) -> &str {
    match stage {
        "draft" => "Draft",
        "planning" => "Planning",
        "critique" => "Critique",
        "approval" => "Approval",
        "building" | "implementation" => "Implementation",
        "review" => "Review",
        "delivered" => "Delivered",
        "research" => "Research",
        other => other,
    }
}

/// Short relative time for the sidebar: `now`, `4 min`, `3 h`, `2 d`.
fn short_ago(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let s = now.saturating_sub(ms) / 1000;
    if s < 45 {
        "now".into()
    } else if s < 3600 {
        format!("{} min", (s as f64 / 60.0).round().max(1.0) as u64)
    } else if s < 86400 {
        format!("{} h", (s as f64 / 3600.0).round() as u64)
    } else {
        format!("{} d", (s as f64 / 86400.0).round() as u64)
    }
}

/// A 28px icon button (the web's `.iconbtn`): ink-2, the hover tint under the pointer.
fn icon_btn<'a, M: Clone + 'a>(name: &str, size: f32, msg: M, pressed: bool) -> Element<'a, M> {
    let p = crate::theme::palette();
    button(
        container(ui::icon(name, size, if pressed { p.ink } else { p.ink_2 })).center(Length::Fill),
    )
    .width(Length::Fixed(28.0))
    .height(Length::Fixed(28.0))
    .padding(0)
    .style(move |_: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: (hovered || pressed).then(|| crate::theme::palette().hover.into()),
            text_color: crate::theme::palette().ink,
            border: Border::default().rounded(7.0),
            ..Default::default()
        }
    })
    .on_press(msg)
    .into()
}

/// The brand mark (the web's `.mark`): four tiles and the signal dot on the action colour, radius 7.
fn mark<'a, M: 'a>() -> Element<'a, M> {
    let p = crate::theme::palette();
    let fg = format!(
        "#{:02x}{:02x}{:02x}",
        (p.on_action.r * 255.0) as u8,
        (p.on_action.g * 255.0) as u8,
        (p.on_action.b * 255.0) as u8
    );
    let dot = format!(
        "#{:02x}{:02x}{:02x}",
        (p.signal.r * 255.0) as u8,
        (p.signal.g * 255.0) as u8,
        (p.signal.b * 255.0) as u8
    );
    let bg = format!(
        "#{:02x}{:02x}{:02x}",
        (p.action.r * 255.0) as u8,
        (p.action.g * 255.0) as u8,
        (p.action.b * 255.0) as u8
    );
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="3" y="3" width="7.8" height="7.8" rx="2.2" fill="{fg}"/><rect x="13.2" y="3" width="7.8" height="7.8" rx="2.2" fill="{fg}" opacity=".55"/><rect x="3" y="13.2" width="7.8" height="7.8" rx="2.2" fill="{fg}" opacity=".55"/><rect x="13.2" y="13.2" width="7.8" height="7.8" rx="2.2" fill="{fg}" opacity=".85"/><circle cx="12" cy="12" r="2.6" fill="{dot}" stroke="{bg}" stroke-width="1.6"/></svg>"#
    );
    container(
        iced::widget::svg(iced::widget::svg::Handle::from_memory(doc.into_bytes()))
            .width(Length::Fixed(15.0))
            .height(Length::Fixed(15.0)),
    )
    .center(Length::Fixed(24.0))
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().action.into()),
        border: Border::default().rounded(7.0),
        ..Default::default()
    })
    .into()
}

/// A project's monogram tile (the web's `pmono`): two letters on one of the six hues, picked by the id's digits.
pub fn pmono<'a, M: 'a>(id: &str, title: &str, size: f32) -> Element<'a, M> {
    let p = crate::theme::palette();
    let words: Vec<String> = title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let first = words.first().and_then(|w| w.chars().next()).unwrap_or('?');
    let second = words
        .get(1)
        .or(words.first())
        .and_then(|w| w.chars().next())
        .unwrap_or(' ');
    let m: String = [first, second].iter().collect::<String>().to_uppercase();
    let digits: u64 = id
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    let hue = [p.hue_0, p.hue_1, p.hue_2, p.hue_3, p.hue_4, p.hue_5][(digits % 6) as usize];
    container(
        text(m)
            .font(ui::SANS_SEMIBOLD)
            .size(size * 0.46)
            .color(p.on_hue),
    )
    .center(Length::Fixed(size))
    .style(move |_: &Theme| container::Style {
        background: Some(hue.into()),
        border: Border::default().rounded(size * 0.28),
        ..Default::default()
    })
    .into()
}

/// A sidebar row (the web's `.nrow`): 32 tall, radius 8; the press tint on the screen on show, the hover tint under
/// the pointer.
fn nrow<'a, M: Clone + 'a>(
    icon_name: &str,
    label: &str,
    active: bool,
    end: Option<Element<'a, M>>,
    msg: M,
) -> Element<'a, M> {
    let p = crate::theme::palette();
    let mut r = row![
        ui::icon(icon_name, 16.0, if active { p.ink } else { p.ink_2 }),
        text(label.to_string()).font(SANS).size(13.5),
        Space::new().width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(end) = end {
        r = r.push(end);
    }
    button(container(r).center_y(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(32.0))
        .padding([0, 8])
        .style(move |_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if active {
                    Some(p.press.into())
                } else if hovered {
                    Some(p.hover.into())
                } else {
                    None
                },
                text_color: if active || hovered { p.ink } else { p.ink_2 },
                border: Border::default().rounded(8.0),
                ..Default::default()
            }
        })
        .on_press(msg)
        .into()
}

/// A line under a project (the web's `.thread.line`): an icon, the text, and what ends it.
fn thread_line<'a, M: Clone + 'a>(
    icon_name: &str,
    label: &str,
    end: Element<'a, M>,
    msg: M,
) -> Element<'a, M> {
    let p = crate::theme::palette();
    button(
        row![
            ui::icon(icon_name, 14.0, p.ink_3),
            text(ui::ellipsize(label, 30))
                .font(SANS)
                .size(12.5)
                .color(p.ink_2),
            Space::new().width(Length::Fill),
            end,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding {
        top: 7.0,
        right: 10.0,
        bottom: 8.0,
        left: 34.0,
    })
    .style(row_press)
    .on_press(msg)
    .into()
}

fn row_press(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then(|| crate::theme::palette().hover.into()),
        text_color: crate::theme::palette().ink,
        border: Border::default().rounded(9.0),
        ..Default::default()
    }
}

/// A task's chip in the sidebar.
fn task_chip<'a, M: 'a>(status: &str) -> Element<'a, M> {
    let (label, tone) = match status {
        "claimed" | "working" => ("Working", ui::Tone::Info),
        "review" => ("Review", ui::Tone::Signal),
        "failed" => ("Failed", ui::Tone::Danger),
        "done" => ("Done", ui::Tone::Ok),
        _ => ("Waiting", ui::Tone::Muted),
    };
    ui::badge(label, tone)
}

fn sidebar<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    if state.mode != Mode::Open {
        return Space::new().width(Length::Fixed(0.0)).into();
    }
    let p = crate::theme::palette();
    let top = container(
        row![
            mark(),
            text("AgentPlaza")
                .font(ui::SANS_SEMIBOLD)
                .size(13.5)
                .color(p.ink)
                .width(Length::Fill),
            icon_btn("square-pen", 16.0, wrap(Msg::OpenNewProject), false),
            icon_btn("panel-left", 16.0, wrap(Msg::ToggleSidebar), false),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 0.0,
        right: 8.0,
        bottom: 0.0,
        left: 16.0,
    })
    .center_y(Length::Fixed(48.0));
    let search = container(
        button(
            row![
                ui::icon("search", 15.0, p.ink_3),
                text("Search or jump to…")
                    .font(SANS)
                    .size(13.0)
                    .color(p.ink_3)
                    .width(Length::Fill),
                kbd("Ctrl K"),
            ]
            .spacing(11)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(32.0))
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 7.0,
        })
        .style(|_: &Theme, _| button::Style {
            background: Some(crate::theme::palette().raised.into()),
            text_color: crate::theme::palette().ink_3,
            border: Border::default()
                .width(1.0)
                .color(crate::theme::palette().rule)
                .rounded(8.0),
            shadow: crate::theme::skin().shadow_card,
            snap: true,
        })
        .on_press(wrap(Msg::OpenPalette)),
    )
    .padding(Padding {
        top: 2.0,
        right: 8.0,
        bottom: 8.0,
        left: 8.0,
    });

    let more_on = state.more || MORE.iter().any(|(id, _, _)| shows(id, &state.current));
    let mut nav = column![].spacing(1);
    for (id, label, ic) in MAIN {
        let end = (*id == Id::Approvals)
            .then(|| state.counts.get("approvals").copied().filter(|n| *n > 0))
            .flatten()
            .map(ui::count_hot);
        nav = nav.push(nrow(
            ic,
            label,
            shows(id, &state.current),
            end,
            wrap(Msg::Go(id.clone())),
        ));
    }
    nav = nav.push(nrow(
        if more_on { "chevron-up" } else { "ellipsis" },
        if more_on { "Less" } else { "More" },
        false,
        None,
        wrap(Msg::ToggleMore),
    ));
    if more_on {
        for (id, label, ic) in MORE {
            nav = nav.push(nrow(
                ic,
                label,
                shows(id, &state.current),
                None,
                wrap(Msg::Go(id.clone())),
            ));
        }
    }
    let nav = container(nav).padding(Padding {
        top: 2.0,
        right: 8.0,
        bottom: 4.0,
        left: 8.0,
    });

    // What is moving: the drafts being talked through, then each live project with its lines.
    let mut tree = column![].spacing(0);
    let drafting: Vec<&serde_json::Value> = state
        .conversations
        .iter()
        .filter(|c| c["project"].is_null() && !c["archived"].as_bool().unwrap_or(false))
        .collect();
    if !drafting.is_empty() {
        tree = tree.push(sec("Talking it through", None));
        for c in drafting {
            let id = c["id"].as_str().unwrap_or("").to_string();
            let rev = c["draft"]["revision"].as_u64();
            let sub = match rev {
                Some(r) => format!("Draft · revision {r}"),
                None => "Talking it through".to_string(),
            };
            let pending = c["pending"].as_u64().unwrap_or(0) > 0;
            tree = tree.push(
                button(
                    column![
                        row![
                            ui::icon("file-pen-line", 14.0, p.ink_3),
                            text(ui::ellipsize(c["title"].as_str().unwrap_or(""), 26))
                                .font(SANS)
                                .size(13.0)
                                .color(p.ink),
                            Space::new().width(Length::Fill),
                            ui::mono(
                                short_ago(c["updated_ms"].as_u64().unwrap_or(0)),
                                11.0,
                                p.ink_3
                            ),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                        {
                            let mut t2 = row![
                                ui::sans(sub, 12.0, p.ink_3),
                                Space::new().width(Length::Fill)
                            ];
                            if pending {
                                t2 = t2.push(ui::badge("Answering", ui::Tone::Info));
                            }
                            t2
                        },
                    ]
                    .spacing(4),
                )
                .width(Length::Fill)
                .padding(Padding {
                    top: 7.0,
                    right: 10.0,
                    bottom: 8.0,
                    left: 12.0,
                })
                .style(row_press)
                .on_press(wrap(Msg::Go(Id::Ide(Some(id))))),
            );
        }
    }
    tree = tree.push(sec(
        "Projects",
        Some(icon_btn("plus", 14.0, wrap(Msg::OpenNewProject), false)),
    ));
    let mut live: Vec<&serde_json::Value> = scope::live_projects(&state.projects);
    live.sort_by_key(|p| std::cmp::Reverse(p["updated_ms"].as_u64().unwrap_or(0)));
    if live.is_empty() {
        tree = tree.push(
            container(ui::sans(
                "No live projects. Start one with the orchestrator.",
                12.5,
                p.ink_3,
            ))
            .padding(Padding {
                top: 6.0,
                right: 16.0,
                bottom: 10.0,
                left: 16.0,
            }),
        );
    }
    for proj in live {
        let id = proj["id"].as_str().unwrap_or("").to_string();
        let title = proj["title"].as_str().unwrap_or(&id).to_string();
        let folded = state.folded.contains(&id);
        let tag: Element<'a, M> = if proj["paused"].as_bool().unwrap_or(false) {
            mode_badge("Paused", ui::Tone::Warn)
        } else if proj["mode"] == "maintenance" {
            mode_badge("Maintenance", ui::Tone::Ok)
        } else {
            ui::mono(
                stage_label(proj["stage"].as_str().unwrap_or("")).to_string(),
                11.0,
                p.ink_3,
            )
            .into()
        };
        let active = matches!(&state.current, Id::Project(x) | Id::ProjectTab(x, _) if *x == id);
        let tw = button(
            container(ui::icon(
                if folded {
                    "chevron-right"
                } else {
                    "chevron-down"
                },
                13.0,
                p.ink_3,
            ))
            .center(Length::Fill),
        )
        .width(Length::Fixed(18.0))
        .height(Length::Fixed(28.0))
        .padding(0)
        .style(|_: &Theme, _| button::Style::default())
        .on_press(wrap(Msg::Fold(id.clone())));
        let head = button(
            row![
                pmono(&id, &title, 18.0),
                text(ui::ellipsize(&title, 22))
                    .font(SANS)
                    .size(12.5)
                    .width(Length::Fill),
                tag,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(30.0))
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 4.0,
        })
        .style(move |_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if active {
                    Some(p.press.into())
                } else if hovered {
                    Some(p.hover.into())
                } else {
                    None
                },
                text_color: if active { p.ink } else { p.ink_2 },
                border: Border::default().rounded(7.0),
                ..Default::default()
            }
        })
        .on_press(wrap(Msg::Go(Id::Project(id.clone()))));
        let mut block = column![
            container(row![tw, head].spacing(2).align_y(Alignment::Center)).padding([0, 8])
        ];
        if !folded {
            if let Some(conv) = state
                .conversations
                .iter()
                .find(|c| c["project"] == id.as_str() && !c["archived"].as_bool().unwrap_or(false))
            {
                let cid = conv["id"].as_str().unwrap_or("").to_string();
                block = block.push(
                    container(thread_line(
                        "messages-square",
                        "With the orchestrator",
                        ui::mono(
                            short_ago(conv["updated_ms"].as_u64().unwrap_or(0)),
                            11.0,
                            p.ink_3,
                        )
                        .into(),
                        wrap(Msg::Go(Id::Ide(Some(cid)))),
                    ))
                    .padding([0, 8]),
                );
            }
            for a in state
                .approvals
                .iter()
                .filter(|a| a["project"] == id.as_str())
                .take(3)
            {
                let label = a["title"]
                    .as_str()
                    .or(a["summary"].as_str())
                    .or(a["kind"].as_str())
                    .unwrap_or("Waiting on you");
                block = block.push(
                    container(thread_line(
                        "shield-check",
                        label,
                        ui::dot(p.signal),
                        wrap(Msg::Go(Id::Approvals)),
                    ))
                    .padding([0, 8]),
                );
            }
            let moving = state.tasks.iter().filter(|t| {
                t["project"] == id.as_str()
                    && !matches!(
                        t["status"].as_str().unwrap_or(""),
                        "done" | "verified" | "failed" | "cancelled" | "withdrawn"
                    )
            });
            for t in moving.take(6) {
                let tid = t["id"].as_str().unwrap_or("").to_string();
                let who = t["agent_cid"].as_u64().map(|cid| {
                    state
                        .agents
                        .iter()
                        .find(|a| a["cid"].as_u64() == Some(cid))
                        .map_or_else(|| roles::agent_label("", cid), roles::agent_label_of)
                });
                let updated = t["updated_ms"]
                    .as_u64()
                    .or(t["updated_at"].as_u64())
                    .unwrap_or(0);
                let mut t2 = row![ui::mono(tid.clone(), 11.0, p.ink_3)]
                    .spacing(8)
                    .align_y(Alignment::Center);
                if let Some(who) = who {
                    t2 = t2.push(ui::sans(who.to_string(), 11.5, p.ink_3));
                }
                t2 = t2
                    .push(Space::new().width(Length::Fill))
                    .push(task_chip(t["status"].as_str().unwrap_or("open")));
                block = block.push(
                    container(
                        button(
                            column![
                                row![
                                    text(ui::ellipsize(t["goal"].as_str().unwrap_or(&tid), 30))
                                        .font(SANS)
                                        .size(12.5)
                                        .color(p.ink)
                                        .width(Length::Fill),
                                    ui::mono(short_ago(updated), 11.0, p.ink_3),
                                ]
                                .spacing(8),
                                t2,
                            ]
                            .spacing(4),
                        )
                        .width(Length::Fill)
                        .padding(Padding {
                            top: 7.0,
                            right: 10.0,
                            bottom: 8.0,
                            left: 34.0,
                        })
                        .style(row_press)
                        .on_press(wrap(Msg::Go(Id::Task(tid)))),
                    )
                    .padding([0, 8]),
                );
            }
        }
        tree = tree.push(block);
    }

    let hub_label = if !state.hub_ok {
        "apid unreachable".to_string()
    } else if state.paused {
        "Held".to_string()
    } else {
        format!("{} working", state.working)
    };
    let stop: Element<'a, M> = if state.paused {
        stop_all("RESUME", true, wrap(Msg::Resume))
    } else {
        stop_all("STOP ALL", false, wrap(Msg::StopAll))
    };
    let me = button(
        container(text("O").font(ui::SANS_SEMIBOLD).size(11.0).color(p.ink)).center(Length::Fill),
    )
    .width(Length::Fixed(24.0))
    .height(Length::Fixed(24.0))
    .padding(0)
    .style(|_: &Theme, status: button::Status| button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                crate::theme::palette().press
            } else {
                crate::theme::palette().raised
            }
            .into(),
        ),
        text_color: crate::theme::palette().ink,
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule_strong)
            .rounded(12.0),
        ..Default::default()
    })
    .on_press(wrap(Msg::OpenMenu(Menu::Owner)));
    let foot = container(
        row![
            row![
                ui::dot(if state.hub_ok { p.ok } else { p.ink_3 }),
                ui::mono(hub_label, 11.0, p.ink_3)
            ]
            .spacing(7)
            .align_y(Alignment::Center)
            .width(Length::Fill),
            icon_btn("code-xml", 16.0, wrap(Msg::OpenIde), false),
            container(me).padding([0, 2]),
            icon_btn(
                "settings",
                16.0,
                wrap(Msg::Go(Id::Models)),
                settings_group(&state.current).is_some()
            ),
            stop,
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 8.0,
        right: 8.0,
        bottom: 8.0,
        left: 12.0,
    })
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        border: Border::default(),
        ..Default::default()
    });

    let col = column![
        top,
        search,
        nav,
        ui::scroll(container(tree).padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 10.0,
            left: 0.0
        }))
        .id(SIDEBAR_SCROLL.clone())
        .height(Length::Fill),
        rule_across(),
        foot,
    ]
    .height(Length::Fill);
    let width = state.width.max(288.0);
    let body = row![
        container(col).width(Length::Fill).height(Length::Fill),
        container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill)).style(
            |_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule.into()),
                ..Default::default()
            }
        ),
    ];
    let body = container(body)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().paper_side.into()),
            ..Default::default()
        });
    // The handle is laid over the sidebar's right edge, so the sidebar's width is unchanged.
    let edge =
        row![Space::new().width(Length::Fill), resize_handle(state, wrap)].height(Length::Fill);
    stack(vec![body.into(), edge.into()])
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .into()
}

/// A 1px rule across.
fn rule_across<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// A sidebar section head (the web's `.sec`), with a button at its right.
fn sec<'a, M: 'a>(label: &str, end: Option<Element<'a, M>>) -> Element<'a, M> {
    let mut r =
        row![ui::section(label), Space::new().width(Length::Fill)].align_y(Alignment::Center);
    if let Some(end) = end {
        r = r.push(end);
    }
    let loom = crate::theme::skin().label_mono;
    container(r)
        .padding(Padding {
            top: if loom { 16.0 } else { 14.0 },
            right: 8.0,
            bottom: 6.0,
            left: 16.0,
        })
        .width(Length::Fill)
        .into()
}

/// A project's mode badge (the web's `.modebadge`).
fn mode_badge<'a, M: 'a>(label: &str, tone: ui::Tone) -> Element<'a, M> {
    let p = crate::theme::palette();
    let (ink, ground) = match tone {
        ui::Tone::Warn => (p.warn, p.warn_soft),
        _ => (p.ok, p.ok_soft),
    };
    let sk = crate::theme::skin();
    container(
        text(sk.label_text(label))
            .font(sk.label_font())
            .size(10.5)
            .color(ink),
    )
    .padding([3, 6])
    .style(move |_: &Theme| container::Style {
        background: Some(ground.into()),
        border: Border::default().rounded(5.0),
        ..Default::default()
    })
    .into()
}

/// A key cap (the web's `kbd`).
fn kbd<'a, M: 'a>(keys: &str) -> Element<'a, M> {
    container(ui::mono(
        keys.to_string(),
        11.0,
        crate::theme::palette().ink_3,
    ))
    .padding([2, 5])
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().raised.into()),
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule_strong)
            .rounded(5.0),
        ..Default::default()
    })
    .into()
}

/// STOP ALL / RESUME in the sidebar's foot (the web's `.stopall`): crit (or ok) on its soft ground, radius 7.
fn stop_all<'a, M: Clone + 'a>(label: &str, resume: bool, msg: M) -> Element<'a, M> {
    let sk = crate::theme::skin();
    button(
        text(label.to_string())
            .font(if sk.label_mono {
                crate::theme::MONO_MEDIUM
            } else {
                ui::SANS_SEMIBOLD
            })
            .size(if sk.label_mono { 10.5 } else { 11.0 }),
    )
    .padding([6, 8])
    .style(move |_: &Theme, status: button::Status| {
        let p = crate::theme::palette();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (ink, ground) = if resume {
            (p.ok, p.ok_soft)
        } else {
            (p.crit, p.crit_soft)
        };
        let loom = crate::theme::skin().label_mono;
        button::Style {
            background: (loom || hovered).then(|| ground.into()),
            text_color: ink,
            border: Border::default()
                .width(1.0)
                .color(if loom {
                    Color::TRANSPARENT
                } else {
                    Color { a: 0.35, ..ink }
                })
                .rounded(7.0),
            ..Default::default()
        }
    })
    .on_press(msg)
    .into()
}

/// The resize handle: a strip at the right edge; a 2 px signal line shows while the pointer is over
/// it or dragging it. Drag clamps 200–420 (in `update`); a double-click resets the default width.
fn resize_handle<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let lit = state.handle_hot || state.drag.is_some();
    let line = container(Space::new().width(Length::Fixed(2.0)).height(Length::Fill))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: if lit {
                Some(crate::theme::palette().signal.into())
            } else {
                None
            },
            ..Default::default()
        });
    let strip = container(line)
        .width(Length::Fixed(HANDLE_W))
        .height(Length::Fill)
        .align_x(alignment::Horizontal::Right);
    mouse_area(strip)
        .interaction(mouse::Interaction::ResizingHorizontally)
        .on_enter(wrap(Msg::HandleEnter))
        .on_exit(wrap(Msg::HandleExit))
        .on_move(move |p| wrap(Msg::HandleMove(p.x)))
        .on_press(wrap(Msg::DragStart))
        .on_release(wrap(Msg::DragEnd))
        .on_double_click(wrap(Msg::ResetWidth))
        .into()
}

/// The head's title for a screen: its sidebar label, else the Lab's, else "Agent".
fn top_bar_title(current: &Id) -> &'static str {
    NAV.iter()
        .find(|e| shows(&e.0, current))
        .map(|e| if e.0 == Id::History { "Weft" } else { e.1 })
        .or_else(|| super::screens::lab::Section::of(current).map(|_| "Lab"))
        .or(match current {
            Id::Ide(_) => Some("IDE"),
            Id::Artifact(_) => Some("Artifact"),
            _ => None,
        })
        .unwrap_or("Agent")
}

/// The head's crumbs for the screen on show: its group and screen in Settings, else the screen's name.
fn crumbs(state: &State) -> Vec<String> {
    if let Some((_, gl, _, items)) = settings_group(&state.current) {
        let id = match &state.current {
            Id::ThemeEditor(_) => &Id::Appearance,
            other => other,
        };
        let label = items
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, l)| *l)
            .unwrap_or(gl);
        return if label == *gl {
            vec!["Settings".into(), gl.to_string()]
        } else {
            vec!["Settings".into(), gl.to_string(), label.to_string()]
        };
    }
    match &state.current {
        Id::Project(id) | Id::ProjectTab(id, _) => vec![scope::title_in(&state.projects, id)],
        Id::Agent(cid) => {
            let label = state
                .agents
                .iter()
                .find(|a| a["cid"].as_u64() == Some(*cid))
                .map_or_else(|| roles::agent_label("", *cid), roles::agent_label_of);
            vec!["Agents".into(), label]
        }
        Id::Task(id) => vec!["Tasks".into(), id.clone()],
        other => vec![top_bar_title(other).to_string()],
    }
}

/// The live line's items: what each working agent does now (the web's `liveItems`).
fn live_items(state: &State) -> Vec<(String, bool, Id)> {
    state
        .agents
        .iter()
        .filter(|a| a["status"] == "working")
        .map(|a| {
            let name = roles::agent_label_of(a);
            let stage = a["step"]["stage"].as_str().unwrap_or("");
            let verb = match stage {
                "approval" => "waits on you".to_string(),
                "tool" => format!("runs {}", a["step"]["tool"].as_str().unwrap_or("a tool")),
                "streaming" => "is writing".to_string(),
                _ => "is thinking".to_string(),
            };
            let on = a["work"]["origin"]["task"].as_str().unwrap_or("chat");
            let to = if stage == "approval" {
                Id::Approvals
            } else {
                Id::Agent(a["cid"].as_u64().unwrap_or(0))
            };
            (format!("{name} {verb} · {on}"), stage == "approval", to)
        })
        .collect()
}

fn top_bar<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let p = crate::theme::palette();
    let mut bar = row![].spacing(8).align_y(Alignment::Center);
    if state.mode != Mode::Open {
        bar = bar.push(icon_btn(
            "panel-left",
            16.0,
            wrap(Msg::ToggleSidebar),
            false,
        ));
    }
    if settings_group(&state.current).is_none() {
        let scoped = state
            .project
            .as_ref()
            .map(|id| (id.clone(), scope::title_in(&state.projects, id)));
        let lead: Element<'a, M> = match &scoped {
            Some((id, title)) => pmono(id, title, 16.0),
            None => ui::icon("layers", 14.0, p.ink_2),
        };
        let pill = button(
            row![
                lead,
                text(ui::ellipsize(
                    scoped
                        .as_ref()
                        .map(|s| s.1.as_str())
                        .unwrap_or("All projects"),
                    24
                ))
                .font(SANS)
                .size(12.5),
                ui::icon("chevrons-up-down", 13.0, p.ink_3),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .height(Length::Fixed(28.0))
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 7.0,
        })
        .style(|_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            button::Style {
                background: Some(
                    if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                        p.hover
                    } else {
                        p.raised
                    }
                    .into(),
                ),
                text_color: p.ink_2,
                border: Border::default().width(1.0).color(p.rule).rounded(8.0),
                ..Default::default()
            }
        })
        .on_press(wrap(Msg::OpenMenu(Menu::Scope)));
        bar = bar.push(pill);
    }
    let parts = crumbs(state);
    let mut crumb = row![].spacing(8).align_y(Alignment::Center);
    if let Id::Project(id) | Id::ProjectTab(id, _) = &state.current {
        crumb = crumb.push(pmono(id, &scope::title_in(&state.projects, id), 18.0));
    }
    let last = parts.len().saturating_sub(1);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            crumb = crumb.push(ui::sans("/", 13.0, p.rule_strong));
        }
        crumb = crumb.push(if i == last {
            text(part.clone())
                .font(ui::SANS_MEDIUM)
                .size(13.5)
                .color(p.ink)
        } else {
            text(part.clone()).font(SANS).size(13.5).color(p.ink_3)
        });
    }
    bar = bar.push(container(crumb).width(Length::Fill));
    let items = live_items(state);
    if !items.is_empty() {
        let (line, needs, to) = items[state.live_at % items.len()].clone();
        let pill = button(
            row![
                ui::dot(if needs { p.signal } else { p.ok }),
                text(ui::ellipsize(&line, 44))
                    .font(SANS)
                    .size(12.5)
                    .color(p.ink_2),
                ui::mono(items.len().to_string(), 10.5, p.ink_3),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .height(Length::Fixed(28.0))
        .padding([0, 10])
        .style(|_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(
                    if hovered {
                        p.raised
                    } else {
                        Color {
                            a: 0.72,
                            ..p.raised
                        }
                    }
                    .into(),
                ),
                text_color: p.ink,
                border: Border::default()
                    .width(1.0)
                    .color(if hovered { p.rule_strong } else { p.rule })
                    .rounded(14.0),
                ..Default::default()
            }
        })
        .on_press(wrap(Msg::Go(to)));
        bar = bar.push(pill);
    }
    // The head is glass over the backdrop (the web's `.top`: the panel at 68%).
    container(bar)
        .padding(Padding {
            top: 0.0,
            right: 12.0,
            bottom: 0.0,
            left: 16.0,
        })
        .center_y(Length::Fixed(48.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(
                Color {
                    a: 0.68,
                    ..crate::theme::palette().panel
                }
                .into(),
            ),
            ..Default::default()
        })
        .into()
}

/// Settings' index (the web's `.snav`): each group, its screens under it, and the note on projects.
fn settings_index<'a, M: Clone + 'a>(current: &Id, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let p = crate::theme::palette();
    let on = match current {
        Id::ThemeEditor(_) => Id::Appearance,
        other => other.clone(),
    };
    let mut col = column![].spacing(1);
    for (_, gl, ic, items) in SETTINGS {
        if items.len() == 1 && items[0].1 == *gl {
            col = col.push(nrow(
                ic,
                gl,
                items[0].0 == on,
                None,
                wrap(Msg::Go(items[0].0.clone())),
            ));
        } else {
            col = col.push(Space::new().height(Length::Fixed(6.0)));
            col = col.push(
                container(
                    row![ui::icon(ic, 16.0, p.ink_3), ui::label(gl)]
                        .spacing(10)
                        .align_y(Alignment::Center),
                )
                .padding([0, 8])
                .center_y(Length::Fixed(32.0)),
            );
            for (id, label) in items.iter() {
                col = col.push(
                    container(nrow("", label, *id == on, None, wrap(Msg::Go(id.clone())))).padding(
                        Padding {
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                            left: 26.0,
                        },
                    ),
                );
            }
        }
    }
    col = col.push(
        container(ui::sans(
            "A project overrides models, permissions and limits in its own Settings tab.",
            12.0,
            p.ink_3,
        ))
        .padding(Padding {
            top: 16.0,
            right: 8.0,
            bottom: 0.0,
            left: 8.0,
        }),
    );
    row![
        container(ui::scroll(container(col).padding([16, 10])).height(Length::Fill))
            .width(Length::Fixed(232.0))
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().paper_side.into()),
                ..Default::default()
            }),
        container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill)).style(
            |_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule.into()),
                ..Default::default()
            }
        ),
    ]
    .into()
}

/// One row of the palette: its group, icon, label, hint and where it goes.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub group: &'static str,
    pub icon: &'static str,
    pub label: String,
    pub hint: String,
    pub to: Id,
}

/// The lists the palette searches.
fn state_lists<'s>(
    projects: &'s [serde_json::Value],
    agents: &'s [serde_json::Value],
) -> (&'s [serde_json::Value], &'s [serde_json::Value]) {
    (projects, agents)
}

/// The palette's rows for `query`: the screens, the live projects, the agents and the settings (the web's palette).
pub fn palette_items(
    lists: (&[serde_json::Value], &[serde_json::Value]),
    query: &str,
) -> Vec<PaletteItem> {
    let (projects, agents) = lists;
    let mut all: Vec<PaletteItem> = MAIN
        .iter()
        .chain(MORE.iter())
        .map(|(id, label, ic)| PaletteItem {
            group: "Go to",
            icon: ic,
            label: label.to_string(),
            hint: "Screen".into(),
            to: id.clone(),
        })
        .collect();
    for p in scope::live_projects(projects) {
        if let Some(id) = p["id"].as_str() {
            all.push(PaletteItem {
                group: "Projects",
                icon: "folder",
                label: p["title"].as_str().unwrap_or(id).to_string(),
                hint: stage_label(p["stage"].as_str().unwrap_or("")).to_string(),
                to: Id::Project(id.to_string()),
            });
        }
    }
    for a in agents {
        if let Some(cid) = a["cid"].as_u64() {
            all.push(PaletteItem {
                group: "Agents",
                icon: "bot",
                label: roles::agent_label_of(a),
                hint: a["role"].as_str().unwrap_or("").to_string(),
                to: Id::Agent(cid),
            });
        }
    }
    for (_, gl, ic, items) in SETTINGS {
        for (id, label) in items.iter() {
            all.push(PaletteItem {
                group: "Settings",
                icon: ic,
                label: if label == gl {
                    label.to_string()
                } else {
                    format!("{gl}: {label}")
                },
                hint: "Settings".into(),
                to: id.clone(),
            });
        }
    }
    let q = query.trim().to_lowercase();
    let shown: Vec<PaletteItem> = if q.is_empty() {
        all.into_iter()
            .filter(|it| it.group == "Go to" || it.group == "Projects")
            .collect()
    } else {
        all.into_iter()
            .filter(|it| {
                it.label.to_lowercase().contains(&q) || it.hint.to_lowercase().contains(&q)
            })
            .collect()
    };
    shown.into_iter().take(40).collect()
}

/// The search field's id: focused when the palette opens.
pub static PALETTE_INPUT: std::sync::LazyLock<iced::widget::Id> =
    std::sync::LazyLock::new(iced::widget::Id::unique);

/// The palette (the web's `.pal`): the field, the rows by group, the keys at its foot; over a scrim.
fn palette_view<'a, M: Clone + 'a>(
    state: &'a State,
    query: &'a str,
    sel: usize,
    wrap: fn(Msg) -> M,
) -> Element<'a, M> {
    let p = crate::theme::palette();
    let items = palette_items(state_lists(&state.projects, &state.agents), query);
    let scope_label = state
        .project
        .as_ref()
        .map(|id| scope::title_in(&state.projects, id))
        .unwrap_or_else(|| "Whole plaza".into());
    let field = container(
        row![
            ui::icon("search", 17.0, p.ink_3),
            text_input("Search projects, agents, screens, or settings…", query)
                .id(PALETTE_INPUT.clone())
                .on_input(move |q| wrap(Msg::PaletteQuery(q)))
                .on_submit(wrap(Msg::PalettePick))
                .font(SANS)
                .size(15.5)
                .padding(0)
                .style(|_: &Theme, _| text_input::Style {
                    background: Color::TRANSPARENT.into(),
                    border: Border::default(),
                    icon: crate::theme::palette().ink_3,
                    placeholder: crate::theme::palette().ink_3,
                    value: crate::theme::palette().ink,
                    selection: crate::theme::palette().signal_soft,
                }),
            container(ui::mono(scope_label, 11.0, p.ink_3))
                .padding([4, 6])
                .style(|_: &Theme| container::Style {
                    border: Border::default()
                        .width(1.0)
                        .color(crate::theme::palette().rule_strong)
                        .rounded(6.0),
                    ..Default::default()
                }),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([0, 16])
    .center_y(Length::Fixed(54.0));
    let mut list = column![].spacing(0);
    let mut group = "";
    for (i, it) in items.iter().enumerate() {
        if it.group != group {
            group = it.group;
            let sk = crate::theme::skin();
            list = list.push(
                container(
                    text(sk.label_text(group))
                        .font(crate::theme::MONO_MEDIUM)
                        .size(10.5)
                        .color(p.ink_3),
                )
                .padding(Padding {
                    top: 12.0,
                    right: 10.0,
                    bottom: 6.0,
                    left: 10.0,
                }),
            );
        }
        let on = i == sel % items.len().max(1);
        let row_el = button(
            row![
                ui::icon(it.icon, 16.0, if on { p.ink } else { p.ink_3 }),
                text(it.label.clone())
                    .font(SANS)
                    .size(13.5)
                    .color(p.ink)
                    .width(Length::Fill),
                ui::sans(it.hint.clone(), 12.0, p.ink_3),
            ]
            .spacing(11)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([7, 10])
        .style(move |_: &Theme, status: button::Status| button::Style {
            background: (on || matches!(status, button::Status::Hovered))
                .then(|| crate::theme::palette().hover.into()),
            text_color: crate::theme::palette().ink,
            border: Border::default().rounded(10.0),
            ..Default::default()
        })
        .on_press(wrap(Msg::PaletteGo(it.to.clone())));
        list = list.push(row_el);
    }
    if items.is_empty() {
        list = list.push(
            container(ui::sans(
                format!("Nothing matches “{}”.", query.trim()),
                13.5,
                p.ink_3,
            ))
            .padding([36, 16])
            .center_x(Length::Fill),
        );
    }
    let foot = container(
        row![
            row![kbd("↑"), kbd("↓"), ui::sans("move", 12.0, p.ink_3)]
                .spacing(6)
                .align_y(Alignment::Center),
            row![kbd("Enter"), ui::sans("open", 12.0, p.ink_3)]
                .spacing(6)
                .align_y(Alignment::Center),
            row![kbd("Esc"), ui::sans("close", 12.0, p.ink_3)]
                .spacing(6)
                .align_y(Alignment::Center),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding([9, 14])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().paper_side.into()),
        border: Border {
            radius: iced::border::Radius {
                top_left: 0.0,
                top_right: 0.0,
                bottom_right: 16.0,
                bottom_left: 16.0,
            },
            ..Border::default()
        },
        ..Default::default()
    });
    let box_ = container(column![
        field,
        rule_across(),
        ui::scroll(container(list).padding(6)).height(Length::Shrink),
        rule_across(),
        foot
    ])
    .width(Length::Fixed(640.0))
    .max_height(560.0)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().raised.into()),
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule_strong)
            .rounded(16.0),
        shadow: crate::theme::skin().shadow_pop,
        ..Default::default()
    });
    mouse_area(
        container(mouse_area(box_).on_press(wrap(Msg::PaletteQuery(query.to_string()))))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .padding(Padding {
                top: 120.0,
                right: 16.0,
                bottom: 16.0,
                left: 16.0,
            })
            .style(crate::theme::scrim),
    )
    .on_press(wrap(Msg::ClosePalette))
    .into()
}

/// The scope menu (below the scope pill) or the owner's menu (above the sidebar's foot).
fn menu_view<'a, M: Clone + 'a>(
    state: &'a State,
    which: Menu,
    wrap: fn(Msg) -> M,
) -> Element<'a, M> {
    let p = crate::theme::palette();
    let item = |icon: Element<'a, M>,
                label: String,
                small: Option<String>,
                current: bool,
                msg: M|
     -> Element<'a, M> {
        let mut words = column![
            text(label)
                .font(SANS)
                .size(13.0)
                .color(crate::theme::palette().ink)
        ];
        if let Some(small) = small {
            words = words.push(ui::sans(small, 11.5, crate::theme::palette().ink_3));
        }
        button(row![icon, words].spacing(10).align_y(Alignment::Center))
            .width(Length::Fill)
            .padding([6, 10])
            .style(move |_: &Theme, status: button::Status| button::Style {
                background: (current || matches!(status, button::Status::Hovered))
                    .then(|| crate::theme::palette().hover.into()),
                text_color: crate::theme::palette().ink,
                border: Border::default().rounded(7.0),
                ..Default::default()
            })
            .on_press(msg)
            .into()
    };
    let hr = || -> Element<'a, M> { container(rule_across()).padding([4, 0]).into() };
    let sk = crate::theme::skin();
    let head = |t: &str| -> Element<'a, M> {
        container(
            text(sk.label_text(t))
                .font(sk.label_font())
                .size(11.0)
                .color(crate::theme::palette().ink_3),
        )
        .padding(Padding {
            top: 6.0,
            right: 10.0,
            bottom: 4.0,
            left: 10.0,
        })
        .into()
    };
    let mut col = column![].spacing(1);
    let (left, top, bottom) = match which {
        Menu::Scope => {
            col = col.push(head("Show"));
            col = col.push(item(
                ui::icon("layers", 15.0, p.ink_2),
                "All projects".into(),
                Some("The whole plaza".into()),
                state.project.is_none(),
                wrap(Msg::Scope(None, None)),
            ));
            col = col.push(hr());
            for c in scope_choices(&state.projects, state.project.as_deref()) {
                if let ScopeChoice::Project(id, title) = c {
                    let stage = state
                        .projects
                        .iter()
                        .find(|p| p["id"] == id.as_str())
                        .map(|p| {
                            if p["mode"] == "maintenance" {
                                "Maintenance".to_string()
                            } else {
                                stage_label(p["stage"].as_str().unwrap_or("")).to_string()
                            }
                        });
                    let current = state.project.as_deref() == Some(id.as_str());
                    col = col.push(item(
                        pmono(&id, &title, 18.0),
                        title.clone(),
                        stage,
                        current,
                        wrap(Msg::Scope(Some(id), None)),
                    ));
                }
            }
            col = col.push(hr());
            col = col.push(item(
                ui::icon("plus", 15.0, p.ink_2),
                "New project…".into(),
                None,
                false,
                wrap(Msg::OpenNewProject),
            ));
            (state.width.max(288.0) + 16.0, Some(54.0), None)
        }
        Menu::Owner => {
            col = col.push(head("Owner of this plaza"));
            col = col.push(item(
                ui::icon("settings", 15.0, p.ink_2),
                "Settings".into(),
                None,
                false,
                wrap(Msg::Go(Id::Models)),
            ));
            col = col.push(item(
                ui::icon("palette", 15.0, p.ink_2),
                "Appearance".into(),
                None,
                false,
                wrap(Msg::Go(Id::Appearance)),
            ));
            col = col.push(item(
                ui::icon("wand-sparkles", 15.0, p.ink_2),
                "Set up AgentPlaza".into(),
                None,
                false,
                wrap(Msg::Go(Id::Setup)),
            ));
            (100.0, None, Some(52.0))
        }
    };
    let menu = container(col)
        .width(Length::Fixed(260.0))
        .padding(4)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().raised.into()),
            border: Border::default()
                .width(1.0)
                .color(crate::theme::palette().rule_strong)
                .rounded(10.0),
            shadow: crate::theme::skin().shadow_pop,
            ..Default::default()
        });
    let placed = container(mouse_area(menu).on_press(wrap(Msg::OpenMenu(which))))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding {
            top: top.unwrap_or(0.0),
            right: 0.0,
            bottom: bottom.unwrap_or(0.0),
            left,
        })
        .align_y(if bottom.is_some() {
            Alignment::End
        } else {
            Alignment::Start
        });
    mouse_area(placed).on_press(wrap(Msg::CloseMenu)).into()
}

/// Under a project, a setup screen says it holds the plaza's settings, with the way to the project's own.
fn settings_note<'a, M: Clone + 'a>(project: &str, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let ink_2 = crate::theme::palette().ink_2;
    container(
        row![
            ui::icon("settings", 14.0, ink_2),
            ui::sans(
                "These are the plaza's settings, which every project starts from.",
                12.5,
                ink_2
            ),
            ui::text_link(
                "This project's own settings",
                wrap(Msg::Go(Id::ProjectTab(project.to_string(), Tab::Settings))),
            ),
            ui::sans("override some of them.", 12.5, ink_2),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Rendering fell back to the CPU: say so, with the adapter, instead of just being slow.
fn gpu_banner<'a, M: 'a>(adapter: &'a str) -> Element<'a, M> {
    container(
        column![
            ui::sans("Software rendering: no GPU is available to the app, so the 3D floor is slow.", 12.5, crate::theme::palette().crit),
            ui::faint(format!("Adapter: {adapter}. A driver update that has not been loaded yet (reboot) is the usual cause.")),
        ]
        .spacing(4),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border { color: crate::theme::palette().crit, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    })
    .into()
}

/// The web's outlined square mark (`base.css`, "Marks"): an 8 px box with a 1 px border in `tone`.
fn hollow_mark<'a, M: 'a>(tone: Color) -> Element<'a, M> {
    container(
        Space::new()
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0)),
    )
    .style(move |_: &Theme| container::Style {
        border: Border {
            color: tone,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// A header button at the web's `.btn` size: 36 px high, 14 px sides, 11.5 px mono, `on` (label and icon) on `fill`
/// with an ink border. `ui::btn` is 29 px high, so the top bar draws its own.
fn top_btn<'a, M: Clone + 'a>(
    label: &str,
    icon_name: &str,
    fill: Color,
    on: Color,
    msg: M,
) -> Element<'a, M> {
    let inner = row![
        ui::icon(icon_name, 14.0, on),
        text(label.to_string())
            .font(ui::SANS_MEDIUM)
            .size(crate::theme::TYPE_UI)
            .color(on)
    ]
    .spacing(7)
    .align_y(Alignment::Center);
    button(inner)
        .padding([10, 14])
        .on_press(msg)
        .style(move |_: &Theme, _: button::Status| button::Style {
            background: Some(fill.into()),
            text_color: on,
            border: Border {
                color: crate::theme::palette().ink,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A square status light and its label, as the web's top bar draws the hub and node.
/// The top bar's word for the events socket, as the web's sidebar says it.
pub fn live_word(up: bool) -> &'static str {
    if up { "Live" } else { "Reconnecting…" }
}

/// The web's relative time: "just now", "42s ago", "5m ago", "3h ago", "2d ago"; a dash when unknown.
fn ago_text(ms: u64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The held banner under the top bar, worded as the web's: what was cancelled, when, by whom and why.
fn held_banner<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let by = if state.held_by.is_empty() {
        String::new()
    } else {
        format!(" by {}", state.held_by)
    };
    let reason = if state.held_note.is_empty() {
        String::new()
    } else {
        format!(": {}", state.held_note)
    };
    let text = format!(
        "The plaza is stopped — {} running items were cancelled {}{}{}. Nothing new starts until you resume.",
        state.held_cancelled,
        ago_text(state.held_since_ms),
        by,
        reason
    );
    container(
        row![
            hollow_mark(crate::theme::palette().crit),
            ui::sans(text, 12.5, crate::theme::palette().crit).width(Length::Fill),
            top_btn(
                "Resume",
                "play",
                crate::theme::palette().signal,
                crate::theme::palette().on_signal,
                wrap(Msg::Resume)
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().crit,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_plaza_frame_sets_the_hold_without_a_reload() {
        let mut s = State::default();
        let held = serde_json::json!({ "paused": true, "since_ms": 1000, "by": "owner", "reason": "held", "cancelled": 3 });
        assert!(!apply_frame(&mut s, "plaza", &held));
        assert!(s.paused);
        assert_eq!((s.held_cancelled, s.held_since_ms), (3, 1000));
        assert_eq!(
            (s.held_by.as_str(), s.held_note.as_str()),
            ("owner", "held")
        );
        let resumed = serde_json::json!({ "paused": false, "since_ms": 0, "by": "", "reason": "", "cancelled": 0 });
        assert!(!apply_frame(&mut s, "plaza", &resumed));
        assert!(!s.paused);
    }

    #[test]
    fn the_switcher_offers_all_projects_the_live_ones_and_new_project() {
        let projects = vec![
            serde_json::json!({ "id": "pr1", "title": "Docs refresh", "stage": "build", "created_ms": 1 }),
            serde_json::json!({ "id": "pr2", "title": "Release 0.9", "stage": "brief", "created_ms": 2 }),
            serde_json::json!({ "id": "pr3", "title": "Old", "stage": "delivered", "created_ms": 3 }),
        ];
        let names = |c: Vec<ScopeChoice>| c.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            names(scope_choices(&projects, None)),
            [
                "All projects",
                "Release 0.9",
                "Docs refresh",
                "New project…"
            ]
        );
        assert_eq!(
            names(scope_choices(&projects, Some("pr3"))),
            [
                "All projects",
                "Release 0.9",
                "Docs refresh",
                "Old",
                "New project…"
            ],
            "a scope on a project that is over still shows"
        );
        assert!(plaza_settings(&Id::Models) && !plaza_settings(&Id::Agents));
    }

    #[test]
    fn events_agent_and_work_frames_ask_for_a_reload_and_others_do_not() {
        let mut s = State::default();
        assert!(apply_frame(
            &mut s,
            "agent",
            &serde_json::json!({ "cid": 1 })
        ));
        assert!(apply_frame(&mut s, "work", &serde_json::json!({})));
        assert!(!apply_frame(&mut s, "transcript", &serde_json::json!({})));
    }
}
