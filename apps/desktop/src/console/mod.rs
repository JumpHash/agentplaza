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
pub mod floor3d;
pub mod client;
pub mod events;
pub mod icons;
pub mod picker;
pub mod screens;
pub mod services;
pub mod setup;
pub mod shell;
pub mod ui;

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
}

#[derive(Debug, Clone)]
pub enum Msg {
    Boot(boot::Msg),
    Shell(shell::Msg),
    Screen(screens::Msg),
    /// The poll timer: the shown screen reloads.
    Tick,
    /// `APZ_SNAPSHOT`: time to capture the window's own frame, and the frame.
    Snapshot,
    Shot(iced::window::Screenshot),
    /// The graphics backend and adapter in use.
    System(iced::system::Information),
}

fn boot() -> (Console, Task<Msg>) {
    let (boot, task) = boot::init();
    let mut console = Console { ctx: None, boot, shell: shell::State::default(), screens: screens::Screens::default() };
    // For development and captures: `APZ_CONNECT=<hub url>` (and `APZ_TOKEN`) skips the chooser,
    // `APZ_SCREEN=<screen label>` opens that screen.
    let info = iced::system::information().map(Msg::System);
    let Ok(url) = std::env::var("APZ_CONNECT") else {
        return (console, Task::batch([task.map(Msg::Boot), info]));
    };
    let ctx = Ctx { client: client::Client::new(&url, None, std::env::var("APZ_TOKEN").ok()) };
    if let Ok(name) = std::env::var("APZ_SCREEN") {
        if let Some((id, ..)) = screens::NAV.iter().find(|(_, label, ..)| label.eq_ignore_ascii_case(&name)) {
            console.shell.current = id.clone();
        }
    }
    let tasks = Task::batch([
        screens::open(&mut console.screens, console.shell.current.clone(), &ctx).map(Msg::Screen),
        shell::load(&ctx).map(Msg::Shell),
        info,
    ]);
    console.ctx = Some(ctx);
    (console, tasks)
}

fn update(console: &mut Console, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Boot(msg) => {
            let (task, connected) = boot::update(&mut console.boot, msg);
            let mut tasks = vec![task.map(Msg::Boot)];
            if let Some(client) = connected {
                let ctx = Ctx { client };
                tasks.push(screens::open(&mut console.screens, console.shell.current.clone(), &ctx).map(Msg::Screen));
                tasks.push(shell::load(&ctx).map(Msg::Shell));
                console.ctx = Some(ctx);
            }
            Task::batch(tasks)
        }
        Msg::Shell(msg) => {
            let Some(ctx) = console.ctx.clone() else { return Task::none() };
            let (task, go) = shell::update(&mut console.shell, msg, &ctx);
            let mut tasks = vec![task.map(Msg::Shell)];
            if let Some(id) = go {
                tasks.push(screens::open(&mut console.screens, id, &ctx).map(Msg::Screen));
            }
            Task::batch(tasks)
        }
        Msg::Screen(msg) => {
            let Some(ctx) = console.ctx.clone() else { return Task::none() };
            if let Some(id) = screens::navigation(&msg) {
                console.shell.current = id.clone();
                return screens::open(&mut console.screens, id, &ctx).map(Msg::Screen);
            }
            screens::update(&mut console.screens, msg, &ctx).map(Msg::Screen)
        }
        Msg::System(info) => {
            eprintln!("agentplaza-desktop: graphics {} on {}", info.graphics_backend, info.graphics_adapter);
            console.shell.software_gpu = is_software(&info.graphics_adapter).then(|| info.graphics_adapter.clone());
            Task::none()
        }
        Msg::Snapshot => iced::window::oldest().and_then(iced::window::screenshot).map(Msg::Shot),
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
            let Some(ctx) = console.ctx.clone() else { return Task::none() };
            Task::batch([
                screens::refresh(&mut console.screens, &console.shell.current, &ctx).map(Msg::Screen),
                shell::load(&ctx).map(Msg::Shell),
            ])
        }
    }
}

fn view(console: &Console) -> Element<'_, Msg> {
    match &console.ctx {
        None => boot::view(&console.boot).map(Msg::Boot),
        Some(ctx) => shell::view(
            &console.shell,
            screens::view(&console.screens, &console.shell.current, ctx).map(Msg::Screen),
            Msg::Shell,
        ),
    }
}

fn subscription(console: &Console) -> Subscription<Msg> {
    let snapshot = match std::env::var("APZ_SNAPSHOT_AFTER").ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(secs) if std::env::var_os("APZ_SNAPSHOT").is_some() => iced::time::every(std::time::Duration::from_secs(secs)).map(|_| Msg::Snapshot),
        None if std::env::var_os("APZ_SNAPSHOT").is_some() => iced::time::every(std::time::Duration::from_secs(8)).map(|_| Msg::Snapshot),
        _ => Subscription::none(),
    };
    let Some(ctx) = console.ctx.as_ref() else {
        return boot::subscription(&console.boot).map(Msg::Boot);
    };
    Subscription::batch([
        snapshot,
        iced::time::every(std::time::Duration::from_secs(4)).map(|_| Msg::Tick),
        shell::subscription(&console.shell).map(Msg::Shell),
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
        .font(include_bytes!("../../../../assets/fonts/MartianMono-Regular.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/MartianMono-Medium.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/MartianMono-Bold.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/SourceSerif4-Regular.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/SourceSerif4-Semibold.ttf").as_slice())
        .font(include_bytes!("../../../../assets/fonts/SourceSerif4-Italic.ttf").as_slice())
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

/// Writes a window screenshot as a PNG (`APZ_SNAPSHOT`).
fn save_png(shot: &iced::window::Screenshot, path: &str) -> Result<(), String> {
    let size = tiny_skia::IntSize::from_wh(shot.size.width, shot.size.height).ok_or("empty frame")?;
    // The frame is straight RGBA; tiny-skia stores premultiplied, so premultiply (opaque anyway).
    let mut data = shot.rgba.to_vec();
    for px in data.chunks_exact_mut(4) {
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
    ["llvmpipe", "lavapipe", "swiftshader", "softpipe", "software", "swrast"].iter().any(|s| a.contains(s))
}
