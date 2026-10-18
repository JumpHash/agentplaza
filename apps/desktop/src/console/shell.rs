//! The shell around every screen: the sidebar (brand, sections, an icon + label per screen, counts;
//! resizable, collapsible to a rail), the top bar (sidebar toggle, the screen title, the project
//! switcher, hub health, STOP ALL / RESUME), the held banner and the New project dialog — as on the
//! web console.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use iced::widget::{
    Space, Stack, button, column, container, mouse_area, pick_list, row, stack, text, text_input,
    tooltip,
};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme, alignment,
    event, mouse,
};

use super::events;
use super::screens::{Id, NAV, projects::Tab};
use super::{Ctx, new_project, scope, toast, ui};
use crate::theme::SANS;

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
    /// Scopes the console to a project (None: All projects) and opens a screen (None: the one on show, or its
    /// list). The console handles it: the scope is its `Ctx`.
    Scope(Option<String>, Option<Id>),
    /// Opens the New project dialog (the switcher's last entry).
    OpenNewProject,
    NewProject(new_project::Msg),
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
            Ok(
                serde_json::json!({ "overview": overview, "control": control, "projects": projects }),
            )
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
            state.current = id.clone();
            (Task::none(), Some(id))
        }
        // The console scopes itself (it owns the `Ctx`); the message never reaches here from it.
        Msg::Scope(..) => (Task::none(), None),
        Msg::OpenNewProject => {
            state.new_project = Some(new_project::Form::default());
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
            let light = !crate::theme::is_light();
            crate::theme::choose(light);
            let _ = super::boot::save_setting(
                "theme",
                serde_json::json!(if light { "light" } else { "dark" }),
            );
            (Task::none(), None)
        }
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
        main = main.push(
            container(screen)
                .padding(16)
                .width(Length::Fill)
                .height(Length::Fill),
        );
        let main = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(ui::paper);
        row![sidebar(state, wrap), main].height(Length::Fill).into()
    };
    let mut layers = Stack::new().push(base);
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
        )
}

/// Screens whose view scrolls its own page (a `scrollable` inside `ui::sheet`), or that handle their own
/// pointer input (a 3D view's canvas). The shell wraps every other screen; add a screen here once its view scrolls itself.
fn manages_scroll(id: &super::screens::Id) -> bool {
    use super::screens::Id;
    matches!(
        id,
        Id::Overview
            | Id::Agents
            | Id::Projects
            | Id::Project(_)
            | Id::ProjectTab(..)
            | Id::Tasks
            | Id::Task(_)
            | Id::Library
            | Id::RobotLab
            | Id::HardwareLab
            | Id::Ide(_)
            | Id::Society
            | Id::Conversations
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
        _ => None,
    });
    // While a resize drag runs, the pointer's moves and the button's release come from anywhere in
    // the window (the pointer can leave the handle), so they are listened to for the drag only.
    let drag = if state.drag.is_some() {
        event::listen_with(drag_event)
    } else {
        Subscription::none()
    };
    Subscription::batch([frames, live, clock, keys, drag])
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
    let (title, body, action): (&str, &str, Element<'a, M>) = match which {
        Confirm::Stop(reason) => (
            "Stop all agents?",
            "Every agent's running work is cancelled and nothing new starts until you resume. Queued work, VMs and workspaces are kept.",
            column![
                ui::label("Reason"),
                text_input("Why (optional)", reason)
                    .on_input(move |s| wrap(Msg::StopReason(s)))
                    .padding(8),
                ui::faint("Optional. Shown on the held banner."),
                row![
                    ui::btn(
                        "Cancel",
                        None,
                        ui::Kind::Ghost,
                        Some(wrap(Msg::CancelConfirm))
                    ),
                    top_btn(
                        "Stop all",
                        "stop",
                        crate::theme::palette().crit,
                        crate::theme::palette().sheet,
                        wrap(Msg::StopGo)
                    ),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ),
        Confirm::Resume => (
            "Resume the plaza?",
            "New work can start again. The items cancelled by the stop stay cancelled.",
            row![
                ui::btn(
                    "Cancel",
                    None,
                    ui::Kind::Ghost,
                    Some(wrap(Msg::CancelConfirm))
                ),
                top_btn(
                    "Resume",
                    "play",
                    crate::theme::palette().signal,
                    Color::WHITE,
                    wrap(Msg::ResumeGo)
                ),
            ]
            .spacing(8)
            .into(),
        ),
    };
    let panel = container(
        column![
            ui::sans(title, 14.0, crate::theme::palette().ink),
            ui::sans(body, 12.0, crate::theme::palette().ink_2),
            action
        ]
        .spacing(14),
    )
    .padding(18)
    .width(Length::Fixed(440.0))
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: crate::theme::palette().ink,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(
                Color {
                    a: 0.5,
                    ..Color::BLACK
                }
                .into(),
            ),
            ..Default::default()
        })
        .into()
}

fn sidebar<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let open = state.mode == Mode::Open;
    let mut list = column![].spacing(2);
    let mut section: Option<&str> = None;
    for (id, label, sec, icon_name) in NAV.iter() {
        if section != Some(*sec) {
            // The web's `.nav-section`: 14 px above each heading, none above the first.
            let top = if section.is_none() { 0.0 } else { 14.0 };
            section = Some(*sec);
            if open {
                list = list.push(container(ui::section(sec)).padding(Padding {
                    top,
                    right: 10.0,
                    bottom: 6.0,
                    left: 10.0,
                }));
            }
        }
        list = list.push(nav_item(state, id, label, icon_name, open, wrap));
    }
    // The scrollbar's rail (6 px) sits in its own gutter, clear of the rows' count badges.
    let list = container(list).padding(Padding {
        top: 0.0,
        right: 8.0,
        bottom: 0.0,
        left: 0.0,
    });
    let mut col = column![
        brand(open),
        ui::scroll(list)
            .id(SIDEBAR_SCROLL.clone())
            .height(Length::Fill)
    ]
    .height(Length::Fill);
    if open {
        col = col.push(footer(state, wrap));
    }
    let width = if open { state.width } else { 60.0 };
    let body = container(col)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet.into()),
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    if !open {
        return body.into();
    }
    // The handle is laid over the sidebar's right edge, so the sidebar's width is unchanged.
    let edge =
        row![Space::new().width(Length::Fill), resize_handle(state, wrap)].height(Length::Fill);
    stack(vec![body.into(), edge.into()])
        .width(Length::Fixed(width))
        .height(Length::Fill)
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

fn brand<'a, M: 'a>(open: bool) -> Element<'a, M> {
    let mark = ui::hive(26.0);
    let head: Element<'a, M> = if open {
        row![
            mark,
            ui::sans("AgentPlaza", 14.0, crate::theme::palette().ink)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    } else {
        mark
    };
    container(head)
        .padding(Padding {
            top: 14.0,
            right: 12.0,
            bottom: 10.0,
            left: 12.0,
        })
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}

fn nav_item<'a, M: Clone + 'a>(
    state: &'a State,
    id: &Id,
    label: &str,
    icon_name: &str,
    open: bool,
    wrap: fn(Msg) -> M,
) -> Element<'a, M> {
    let active = shows(id, &state.current);
    // The entry the tour is describing is marked as its screen is, on the web too.
    let marked = state.highlight.as_ref() == Some(id);
    let lit = active || marked;
    let glyph = if lit {
        crate::theme::palette().signal
    } else {
        crate::theme::palette().ink_3
    };
    // The web's `.nav a`: a 3 px signal bar inside the row on the active screen; the row takes the
    // active tint (`--hover`) or the hover tint (`--rule-faint`); labels are ink when hot or active.
    let bar = container(Space::new().width(Length::Fixed(3.0)).height(Length::Fill))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: if lit {
                Some(crate::theme::palette().signal.into())
            } else {
                None
            },
            ..Default::default()
        });
    let content: Element<'a, M> = if open {
        let mut r = row![
            bar,
            Space::new().width(Length::Fixed(7.0)),
            ui::icon(icon_name, 16.0, glyph),
            Space::new().width(Length::Fixed(10.0)),
            text(label.to_string()).font(SANS).size(13.0),
            Space::new().width(Length::Fill),
        ]
        .align_y(Alignment::Center)
        .height(Length::Fill);
        if let Some(n) = state.counts.get(label.to_lowercase().as_str()) {
            r = r.push(ui::count(*n));
        }
        r.into()
    } else {
        row![
            bar,
            container(ui::icon(icon_name, 16.0, glyph))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        ]
        .height(Length::Fill)
        .into()
    };
    let item = button(content)
        .width(Length::Fill)
        .height(Length::Fixed(34.0))
        .padding(0)
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let (background, text_color) = if lit {
                (
                    Some(crate::theme::palette().hover.into()),
                    crate::theme::palette().ink,
                )
            } else if hovered {
                (
                    Some(crate::theme::palette().rule_faint.into()),
                    crate::theme::palette().ink,
                )
            } else {
                (None, crate::theme::palette().ink_2)
            };
            button::Style {
                background,
                text_color,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press(wrap(Msg::Go(id.clone())));
    let line: Element<'a, M> = item.into();
    if open {
        line
    } else {
        tooltip(
            line,
            container(ui::sans(
                label.to_string(),
                11.0,
                crate::theme::palette().ink,
            ))
            .padding([4, 8])
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().sheet.into()),
                border: Border {
                    color: crate::theme::palette().ink,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }),
            tooltip::Position::Right,
        )
        .into()
    }
}

/// The sidebar's footer: the hub's address, and the width steps.
fn footer<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    column![
        ui::mono(state.hub.clone(), 10.5, crate::theme::palette().ink_3),
        row![
            Space::new().width(Length::Fill),
            ui::btn("-", None, ui::Kind::Ghost, Some(wrap(Msg::Resize(-24.0)))),
            ui::btn("+", None, ui::Kind::Ghost, Some(wrap(Msg::Resize(24.0)))),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .padding(Padding {
        top: 10.0,
        right: 12.0,
        bottom: 12.0,
        left: 12.0,
    })
    .into()
}

/// The top bar's title for a screen: its sidebar label, else the Lab's, else "Agent".
fn top_bar_title(current: &Id) -> &'static str {
    NAV.iter()
        .find(|e| shows(&e.0, current))
        .map(|e| e.1)
        .or_else(|| super::screens::lab::Section::of(current).map(|_| "Lab"))
        .or(match current {
            super::screens::Id::RobotLab => Some("Robot lab"),
            super::screens::Id::HardwareLab => Some("Hardware lab"),
            super::screens::Id::Ide(_) => Some("IDE"),
            _ => None,
        })
        .unwrap_or("Agent")
}

fn top_bar<'a, M: Clone + 'a>(state: &'a State, wrap: fn(Msg) -> M) -> Element<'a, M> {
    let toggle = ui::btn(
        "",
        Some("panel-left"),
        ui::Kind::Ghost,
        Some(wrap(Msg::ToggleSidebar)),
    );
    let title = top_bar_title(&state.current);
    let health = row![
        indicator(state.hub_ok, "HUB"),
        indicator(state.node_ok, "NODE"),
        indicator(state.live, live_word(state.live)),
        row![
            hollow_mark(crate::theme::palette().ink_2),
            ui::sans(
                format!("{} WORKING · {} IDLE", state.working, state.idle),
                11.5,
                crate::theme::palette().ink_2
            )
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    let mut right = row![health].spacing(16).align_y(Alignment::Center);
    right = right.push(top_btn(
        "Open IDE",
        "square-code",
        crate::theme::palette().signal,
        Color::WHITE,
        wrap(Msg::OpenIde),
    ));
    let (theme_label, theme_icon) = if crate::theme::is_light() {
        ("Dark theme", "moon")
    } else {
        ("Light theme", "sun")
    };
    right = right.push(ui::btn(
        theme_label,
        Some(theme_icon),
        ui::Kind::Ghost,
        Some(wrap(Msg::ToggleTheme)),
    ));
    if state.paused {
        right = right.push(
            row![
                ui::sans("◆", 11.0, crate::theme::palette().crit),
                ui::sans("HELD", 11.5, crate::theme::palette().crit)
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        );
        right = right.push(top_btn(
            "Resume",
            "play",
            crate::theme::palette().signal,
            Color::WHITE,
            wrap(Msg::Resume),
        ));
    } else {
        right = right.push(top_btn(
            "Stop all",
            "stop",
            crate::theme::palette().crit,
            crate::theme::palette().sheet,
            wrap(Msg::StopAll),
        ));
    }
    // The project switcher: All projects, each live project, New project….
    let choices = scope_choices(&state.projects, state.project.as_deref());
    let selected = match &state.project {
        None => ScopeChoice::All,
        Some(id) => choices
            .iter()
            .find(|c| matches!(c, ScopeChoice::Project(pid, _) if pid == id))
            .cloned()
            .unwrap_or_else(|| ScopeChoice::Project(id.clone(), id.clone())),
    };
    let switcher = pick_list(choices, Some(selected), move |choice| {
        wrap(match choice {
            ScopeChoice::All => Msg::Scope(None, None),
            ScopeChoice::Project(id, _) => Msg::Scope(Some(id), None),
            ScopeChoice::New => Msg::OpenNewProject,
        })
    })
    .style(crate::theme::pick_list)
    .text_size(12.5)
    .font(SANS)
    .padding([6, 10]);
    container(
        row![
            toggle,
            ui::sans(title.to_string(), 13.0, crate::theme::palette().ink),
            switcher,
            Space::new().width(Length::Fill),
            right
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
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

fn indicator<'a, M: 'a>(up: bool, label: &'static str) -> Element<'a, M> {
    let tone = if up {
        crate::theme::palette().ok
    } else {
        crate::theme::palette().crit
    };
    row![
        container(
            Space::new()
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(8.0))
        )
        .style(move |_: &Theme| container::Style {
            background: Some(tone.into()),
            ..Default::default()
        }),
        ui::sans(label, 11.5, crate::theme::palette().ink_2),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
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
                Color::WHITE,
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
