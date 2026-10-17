//! The console: the desktop app at parity with the web console.
//!
//! It opens on the startup chooser ([`boot`]): start locally, host a hub, join a hub as a node, or
//! connect to a hub. Once connected it shows the shell ([`shell`]: sidebar, top bar with STOP ALL,
//! the held banner) around one screen at a time ([`screens`]), in the design language of
//! `apps/web/DESIGN.md` ([`ui`]). Every screen talks to the hub through [`client`]. The legacy
//! register (fleet, runs, map, telemetry) stays reachable with `--legacy`.

pub mod boot;
pub mod brands;
pub mod cli_link;
// The 3D engine (`crates/apz-engine`), shared with the web's wasm build.
pub use apz_engine as engine;
pub mod chart;
pub mod client;
pub mod events;
pub mod floor_engine;
pub mod hubs;
pub mod icons;
pub mod lab_api;
pub mod onboard;
pub mod picker;
// The rich content of agent text: chips, hover cards, inline images (docs/design/workspace.md, section 3).
pub(crate) mod render_card;
pub mod rich;
pub mod screens;
pub mod services;
pub mod setup;
pub mod shell;
pub mod toast;
pub mod ui;
pub mod workstation_setup;

use iced::{Element, Subscription, Task};

/// What every screen needs: the hub client.
#[derive(Clone)]
pub struct Ctx {
    pub client: client::Client,
}

pub struct Console {
    /// None until the chooser connected to a hub.
    pub ctx: Option<Ctx>,
    pub boot: boot::State,
    pub shell: shell::State,
    pub screens: screens::Screens,
    /// `APZ_PICK=<machine id>` (a capture hook): the machine the Machines screen picks once after its first load.
    pick: Option<String>,
    pub onboard: onboard::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Boot(boot::Msg),
    Shell(shell::Msg),
    Screen(screens::Msg),
    /// The first-run wizard and the tour.
    Onboard(onboard::Msg),
    /// The poll timer: the shown screen reloads.
    Tick,
    /// `APZ_SNAPSHOT`: time to capture the window's own frame, and the frame.
    Snapshot,
    Shot(iced::window::Screenshot),
    /// The graphics backend and adapter in use.
    System(iced::system::Information),
}

fn boot() -> (Console, Task<Msg>) {
    crate::theme::choose_saved(
        boot::saved_setting("theme")
            .as_ref()
            .and_then(|value| value.as_str()),
    );
    crate::theme::set_machine_style(crate::theme::saved_machine_style(
        boot::saved_setting("machine_style")
            .as_ref()
            .and_then(|value| value.as_str()),
    ));
    let (boot, task) = boot::init();
    if let Some(point) = snapshot_hover() {
        chart::set_snapshot_hover(point);
    }
    let pick = std::env::var("APZ_PICK")
        .ok()
        .filter(|id| !id.trim().is_empty());
    let mut console = Console {
        ctx: None,
        boot,
        shell: shell::State::default(),
        screens: screens::Screens::default(),
        pick,
        onboard: onboard::State::default(),
    };
    // For development and captures: `APZ_CONNECT=<hub url>` (and `APZ_TOKEN`) skips the chooser,
    // `APZ_SCREEN=<screen label>` opens that screen.
    let info = Task::batch([
        iced::system::information().map(Msg::System),
        iced::system::theme().map(|mode| Msg::Shell(shell::Msg::Appearance(mode))),
    ]);
    let Ok(url) = std::env::var("APZ_CONNECT") else {
        return (console, Task::batch([task.map(Msg::Boot), info]));
    };
    let ctx = Ctx {
        client: client::Client::new(&url, None, std::env::var("APZ_TOKEN").ok()),
    };
    if let Ok(name) = std::env::var("APZ_SCREEN") {
        if let Some((id, ..)) = screens::NAV
            .iter()
            .find(|(_, label, ..)| label.eq_ignore_ascii_case(&name))
        {
            console.shell.current = id.clone();
        } else if name.eq_ignore_ascii_case("Floor") {
            // The Floor is the Machines screen's grid, which is its default view.
            console.shell.current = screens::Id::Machines;
        } else if name.eq_ignore_ascii_case("LabJobs") {
            // A Lab section has no nav entry; this opens the Jobs tab for the snapshot harness.
            console.shell.current = screens::Id::LabJobs;
        } else if let Some(id) = [
            ("RobotLab", screens::Id::RobotLab),
            ("HardwareLab", screens::Id::HardwareLab),
        ]
        .into_iter()
        .find_map(|(n, id)| name.eq_ignore_ascii_case(n).then_some(id))
        {
            // The engine's labs are hidden from the sidebar, as the web's lab routes are.
            console.shell.current = id;
        } else if name.eq_ignore_ascii_case("Ide") {
            // The IDE is not in the sidebar; `APZ_IDE_WS=<workspace id>` opens that workspace on it.
            console.shell.current = screens::Id::Ide(
                std::env::var("APZ_IDE_WS")
                    .ok()
                    .filter(|ws| !ws.trim().is_empty()),
            );
        }
    }
    // `APZ_AGENT=<cid>` opens that agent's page: its `Id::Agent` is the current screen, which `screens::open` loads below.
    if let Some(cid) = std::env::var("APZ_AGENT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        console.shell.current = screens::Id::Agent(cid);
    }
    // `APZ_JOB=<id>` opens that Lab job's page: its `Id::LabJobDetail` is the current screen, as `APZ_AGENT` does for an agent.
    if let Ok(job) = std::env::var("APZ_JOB") {
        console.shell.current = screens::Id::LabJobDetail(job);
    }
    // `APZ_LAB_MODEL=<model id>`, `APZ_SCREEN=LabModels|LabBreeders|LabRecipes|LabDatasets` open the Lab screens the sidebar does not list (capture hooks).
    if let Ok(model) = std::env::var("APZ_LAB_MODEL") {
        console.shell.current = screens::Id::LabModelDetail(model);
    } else if std::env::var("APZ_SCREEN").is_ok_and(|s| s == "LabModels") {
        console.shell.current = screens::Id::LabModels;
    } else if std::env::var("APZ_SCREEN").is_ok_and(|s| s == "LabBreeders") {
        console.shell.current = screens::Id::LabBreeders;
    } else if std::env::var("APZ_SCREEN").is_ok_and(|s| s == "LabRecipes") {
        console.shell.current = screens::Id::LabRecipes;
    } else if std::env::var("APZ_SCREEN").is_ok_and(|s| s == "LabDatasets") {
        console.shell.current = screens::Id::LabDatasets;
    }
    // `APZ_DIALOG=vet|export|upload|plan` opens that Lab dialog prefilled on its screen (capture hooks; `plan` previews the Jobs form);
    // `APZ_DIALOG_FILE=<path>` is the JSONL file the upload dialog shows as picked.
    match std::env::var("APZ_DIALOG").ok().as_deref() {
        Some("vet") => {
            console.shell.current = screens::Id::LabRecipes;
            screens::lab_recipes::capture_vet(&mut console.screens.lab_recipes);
        }
        Some("export") => {
            console.shell.current = screens::Id::LabDatasets;
            screens::lab_datasets::capture(
                &mut console.screens.lab_datasets,
                screens::lab_datasets::Capture::Export,
            );
        }
        Some("upload") => {
            console.shell.current = screens::Id::LabDatasets;
            let file = std::env::var("APZ_DIALOG_FILE")
                .ok()
                .map(std::path::PathBuf::from);
            screens::lab_datasets::capture(
                &mut console.screens.lab_datasets,
                screens::lab_datasets::Capture::Upload(file),
            );
        }
        Some("plan") => {
            console.shell.current = screens::Id::LabJobs;
            screens::lab_jobs::capture_plan(&mut console.screens.lab_jobs);
        }
        _ => {}
    }
    // `APZ_VIEW=cards` opens the Lab Jobs list in its cards view (a capture hook, as `APZ_DIALOG`).
    if std::env::var("APZ_VIEW").is_ok_and(|v| v == "cards") {
        console.shell.current = screens::Id::LabJobs;
        screens::lab_jobs::capture_cards(&mut console.screens.lab_jobs);
    }
    let first = onboard::first_start(&mut console.onboard, &ctx).map(Msg::Onboard);
    // `APZ_ONBOARD=tour|checks|deps|off`: the tour, the wizard's last step, its dependencies step, or
    // neither, for captures.
    let mut capture = Task::none();
    match std::env::var("APZ_ONBOARD").as_deref() {
        Ok("tour") => {
            console.onboard.wizard = None;
            onboard::start_tour(&mut console.onboard);
            console.shell.highlight = onboard::highlighted(&console.onboard);
        }
        Ok("off") => console.onboard.wizard = None,
        Ok("checks") | Ok("deps") => {
            let step = std::env::var("APZ_ONBOARD").unwrap_or_default();
            console.onboard.wizard = Some(onboard::step_index(&step));
            capture = onboard::local_task().map(|checks| Msg::Onboard(onboard::Msg::Local(checks)));
        }
        _ => {}
    }
    let reveal = onboard::reveal::<Msg>(&console.onboard, true);
    let tasks = Task::batch([
        screens::open(&mut console.screens, console.shell.current.clone(), &ctx).map(Msg::Screen),
        shell::load(&ctx).map(Msg::Shell),
        first,
        capture,
        reveal,
        info,
    ]);
    console.ctx = Some(ctx);
    (console, tasks)
}

fn update(console: &mut Console, msg: Msg) -> Task<Msg> {
    let before = console.shell.current.clone();
    let mut task = update_app(console, msg);
    // The screen changed: the IDE is told when it is left or entered (its terminals and polling follow).
    if console.shell.current != before
        && let Some(ctx) = console.ctx.clone()
    {
        let hook = screens::changed(&mut console.screens, &before, &console.shell.current, &ctx)
            .map(Msg::Screen);
        task = Task::batch([task, hook]);
    }
    // The IDE remembers the screen it was opened from: its Escape goes back there.
    if matches!(console.shell.current, screens::Id::Ide(_))
        && !matches!(before, screens::Id::Ide(_))
    {
        console.screens.ide.origin = Some(before);
    }
    task
}

fn update_app(console: &mut Console, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Boot(msg) => {
            let (task, connected) = boot::update(&mut console.boot, msg);
            let mut tasks = vec![task.map(Msg::Boot)];
            if let Some(client) = connected {
                let ctx = Ctx { client };
                tasks.push(
                    screens::open(&mut console.screens, console.shell.current.clone(), &ctx)
                        .map(Msg::Screen),
                );
                tasks.push(shell::load(&ctx).map(Msg::Shell));
                tasks.push(onboard::first_start(&mut console.onboard, &ctx).map(Msg::Onboard));
                console.ctx = Some(ctx);
            }
            Task::batch(tasks)
        }
        Msg::Shell(msg) => {
            let Some(ctx) = console.ctx.clone() else {
                return Task::none();
            };
            let (task, go) = shell::update(&mut console.shell, msg, &ctx);
            let mut tasks = vec![task.map(Msg::Shell)];
            if let Some(id) = go {
                tasks.push(screens::open(&mut console.screens, id, &ctx).map(Msg::Screen));
            }
            Task::batch(tasks)
        }
        Msg::Onboard(msg) => {
            let Some(ctx) = console.ctx.clone() else {
                return Task::none();
            };
            let (task, effect) = onboard::update(&mut console.onboard, msg);
            console.shell.highlight = onboard::highlighted(&console.onboard);
            let mut tasks = vec![
                task.map(Msg::Onboard),
                onboard::reveal::<Msg>(&console.onboard, console.shell.mode == shell::Mode::Open),
            ];
            match effect {
                onboard::Effect::None => {}
                onboard::Effect::Navigate(id) => {
                    console.shell.current = id.clone();
                    tasks.push(screens::open(&mut console.screens, id, &ctx).map(Msg::Screen));
                }
                // "Change hub": the chooser again; the wizard waits and resumes once connected.
                onboard::Effect::Chooser => {
                    console.ctx = None;
                    console.boot = boot::init().0;
                }
            }
            Task::batch(tasks)
        }
        // The IDE's Escape and Console button: back to the screen it was opened from (the console owns navigation).
        Msg::Screen(screens::Msg::Ide(screens::ide::Msg::Back)) => {
            let Some(ctx) = console.ctx.clone() else {
                return Task::none();
            };
            let target = screens::ide::back_target(console.screens.ide.origin.clone());
            console.shell.current = target.clone();
            screens::open(&mut console.screens, target, &ctx).map(Msg::Screen)
        }
        // The IDE's chrome offers the theme switch and the held strip's Resume: the shell owns both.
        Msg::Screen(screens::Msg::Ide(screens::ide::Msg::Shell(msg))) => {
            update_app(console, Msg::Shell(msg))
        }
        // The Setup screen replays the wizard or the tour (the console owns both).
        Msg::Screen(screens::Msg::Setup(screens::setup::Msg::Wizard)) => {
            let Some(ctx) = console.ctx.clone() else {
                return Task::none();
            };
            onboard::start_wizard(&mut console.onboard, &ctx).map(Msg::Onboard)
        }
        Msg::Screen(screens::Msg::Setup(screens::setup::Msg::Tour)) => {
            onboard::start_tour(&mut console.onboard);
            console.shell.highlight = onboard::highlighted(&console.onboard);
            onboard::reveal::<Msg>(&console.onboard, console.shell.mode == shell::Mode::Open)
        }
        Msg::Screen(msg) => {
            let Some(ctx) = console.ctx.clone() else {
                return Task::none();
            };
            if let Some(id) = screens::navigation(&msg) {
                console.shell.current = id.clone();
                return screens::open(&mut console.screens, id, &ctx).map(Msg::Screen);
            }
            // `APZ_PICK`: the first load of the Machines screen picks that machine, as a topology click does.
            let loaded = matches!(
                msg,
                screens::Msg::Machines(screens::machines::Msg::Loaded(Ok(_)))
            );
            let pick = if loaded { console.pick.take() } else { None };
            let task = screens::update(&mut console.screens, msg, &ctx).map(Msg::Screen);
            match pick {
                Some(id) => Task::batch([task, pick_after_layout(id)]),
                None => task,
            }
        }
        Msg::System(info) => {
            eprintln!(
                "agentplaza-desktop: graphics {} on {}",
                info.graphics_backend, info.graphics_adapter
            );
            console.shell.software_gpu =
                is_software(&info.graphics_adapter).then(|| info.graphics_adapter.clone());
            Task::none()
        }
        Msg::Snapshot => {
            let capture = iced::window::oldest()
                .and_then(iced::window::screenshot)
                .map(Msg::Shot);
            match snapshot_scroll() {
                Some(dy) => iced::advanced::widget::operate(ScrollBy(dy))
                    .discard()
                    .chain(capture),
                None => capture,
            }
        }
        Msg::Shot(shot) => {
            if let Ok(path) = std::env::var("APZ_SNAPSHOT") {
                match save_png(&shot, &path) {
                    Ok(()) => eprintln!("agentplaza-desktop: saved {path}"),
                    Err(err) => eprintln!("agentplaza-desktop: snapshot failed: {err}"),
                }
            }
            iced::exit()
        }
        Msg::Tick => {
            let Some(ctx) = console.ctx.clone() else {
                return Task::none();
            };
            Task::batch([
                screens::refresh(&mut console.screens, &console.shell.current, &ctx)
                    .map(Msg::Screen),
                shell::load(&ctx).map(Msg::Shell),
            ])
        }
    }
}

/// The `APZ_PICK` pick, sent after a short wait so the screen has laid its sections out (the pick measures them).
fn pick_after_layout(id: String) -> Task<Msg> {
    Task::perform(
        async move {
            std::thread::sleep(std::time::Duration::from_millis(400));
            id
        },
        |id| Msg::Screen(screens::Msg::Machines(screens::machines::Msg::Pick(id))),
    )
}

fn view(console: &Console) -> Element<'_, Msg> {
    match &console.ctx {
        None => boot::view(&console.boot).map(Msg::Boot),
        Some(ctx) => {
            // The wizard takes the window; the tour and its offer lay over the shell.
            if console.onboard.wizard.is_some() {
                return onboard::wizard_view(&console.onboard, ctx.client.base()).map(Msg::Onboard);
            }
            // The IDE's header shows the plaza's hold and the events link, which the shell keeps.
            let plaza = screens::ide::Plaza {
                held: console.shell.paused,
                live: console.shell.live,
            };
            let base = shell::view(
                &console.shell,
                screens::view(&console.screens, &console.shell.current, ctx, plaza)
                    .map(Msg::Screen),
                screens::scrolls_itself(&console.screens, &console.shell.current),
                Msg::Shell,
            );
            let open = console.shell.mode == shell::Mode::Open;
            let width = if open { console.shell.width } else { 60.0 };
            match onboard::overlay(&console.onboard, width, open) {
                Some(layer) => iced::widget::stack([base, layer.map(Msg::Onboard)]).into(),
                None => base,
            }
        }
    }
}

fn subscription(console: &Console) -> Subscription<Msg> {
    let snapshot = match std::env::var("APZ_SNAPSHOT_AFTER")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(secs) if std::env::var_os("APZ_SNAPSHOT").is_some() => {
            iced::time::every(std::time::Duration::from_secs(secs)).map(|_| Msg::Snapshot)
        }
        None if std::env::var_os("APZ_SNAPSHOT").is_some() => {
            iced::time::every(std::time::Duration::from_secs(8)).map(|_| Msg::Snapshot)
        }
        _ => Subscription::none(),
    };
    let Some(ctx) = console.ctx.as_ref() else {
        return boot::subscription(&console.boot).map(Msg::Boot);
    };
    Subscription::batch([
        snapshot,
        iced::time::every(std::time::Duration::from_secs(4)).map(|_| Msg::Tick),
        shell::subscription(&console.shell).map(Msg::Shell),
        iced::system::theme_changes().map(|mode| Msg::Shell(shell::Msg::Appearance(mode))),
        screens::subscription(&console.screens, &console.shell.current, ctx).map(Msg::Screen),
    ])
}

/// Runs the console until the window closes.
pub fn run() -> iced::Result {
    iced::application(boot, update, view)
        .title(|_: &Console| "AgentPlaza".to_string())
        .theme(|_: &Console| crate::theme::theme())
        .subscription(subscription)
        // The register's faces (assets/fonts, SIL OFL 1.1): embedded, so the app needs no install.
        // Inter is the interface and reading face, and the default font; Martian Mono is for code only.
        .default_font(crate::theme::SANS)
        .font(include_bytes!("../../../../assets/fonts/Inter-Regular.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/Inter-Medium.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/Inter-SemiBold.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/Inter-Bold.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/Inter-Italic.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/MartianMono-Regular.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/MartianMono-Medium.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/MartianMono-Bold.ttf").as_slice())
        // With `APZ_SNAPSHOT` the window stays hidden: the app captures its own frame.
        .window(iced::window::Settings {
            size: snapshot_size().unwrap_or(iced::Size::new(1360.0, 880.0)),
            visible: std::env::var_os("APZ_SNAPSHOT").is_none(),
            // The app icon, shown on the window and in the taskbar or dock.
            icon: iced::window::icon::from_file_data(
                include_bytes!("../../../../packaging/icons/agentplaza-256.png"),
                None,
            )
            .ok(),
            platform_specific: platform_specific(),
            ..Default::default()
        })
        .antialiasing(true)
        .run()
}

/// Wayland app id / X11 class: matches the .desktop file name so docks show the icon.
#[cfg(target_os = "linux")]
fn platform_specific() -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific {
        application_id: "agentplaza".to_string(),
        ..Default::default()
    }
}

/// Nothing platform-specific to set off Linux.
#[cfg(not(target_os = "linux"))]
fn platform_specific() -> iced::window::settings::PlatformSpecific {
    iced::window::settings::PlatformSpecific::default()
}

/// `APZ_SNAPSHOT_SIZE=WxH`: the hidden window's size for a snapshot (e.g. a tall one for a long page).
fn snapshot_size() -> Option<iced::Size> {
    std::env::var_os("APZ_SNAPSHOT")?;
    let value = std::env::var("APZ_SNAPSHOT_SIZE").ok()?;
    let (w, h) = value.split_once('x')?;
    let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
    (w >= 320.0 && h >= 240.0 && w <= 8192.0 && h <= 8192.0).then(|| iced::Size::new(w, h))
}

/// `APZ_SNAPSHOT_SCROLL=<px>`: before the capture, scrolls every scrollable that overflows its
/// bounds down by this many pixels, so a long page's lower section can be framed. Read only by the
/// snapshot path.
fn snapshot_scroll() -> Option<f32> {
    std::env::var_os("APZ_SNAPSHOT")?;
    let value = std::env::var("APZ_SNAPSHOT_SCROLL").ok()?;
    value.trim().parse::<f32>().ok().filter(|dy| *dy > 0.0)
}

/// `APZ_HOVER=<x>,<y>`: the window point the snapshot hovers the charts at, as a cursor there would
/// (a chart shows its tooltip at that point). Read only by the snapshot path.
fn snapshot_hover() -> Option<iced::Point> {
    std::env::var_os("APZ_SNAPSHOT")?;
    let value = std::env::var("APZ_HOVER").ok()?;
    let (x, y) = value.split_once(',')?;
    Some(iced::Point::new(
        x.trim().parse::<f32>().ok()?,
        y.trim().parse::<f32>().ok()?,
    ))
}

/// Scrolls the overflowing scrollables by `.0` pixels (the snapshot path only).
struct ScrollBy(f32);

impl iced::advanced::widget::Operation for ScrollBy {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        bounds: iced::Rectangle,
        content_bounds: iced::Rectangle,
        _translation: iced::Vector,
        state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if content_bounds.height > bounds.height {
            state.scroll_by(
                iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: self.0 },
                bounds,
                content_bounds,
            );
        }
    }
}

/// Writes a window screenshot as a PNG (`APZ_SNAPSHOT`).
fn save_png(shot: &iced::window::Screenshot, path: &str) -> Result<(), String> {
    let size =
        tiny_skia::IntSize::from_wh(shot.size.width, shot.size.height).ok_or("empty frame")?;
    // The frame is straight RGBA; tiny-skia stores premultiplied, so premultiply (opaque anyway).
    let mut data = shot.rgba.to_vec();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    let pixmap = tiny_skia::Pixmap::from_vec(data, size).ok_or("bad frame")?;
    pixmap.save_png(path).map_err(|e| e.to_string())
}

/// Whether a graphics adapter name is a CPU (software) renderer.
fn is_software(adapter: &str) -> bool {
    let a = adapter.to_ascii_lowercase();
    [
        "llvmpipe",
        "lavapipe",
        "swiftshader",
        "softpipe",
        "software",
        "swrast",
    ]
    .iter()
    .any(|s| a.contains(s))
}
