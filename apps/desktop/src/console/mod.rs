//! The console: the desktop app at parity with the web console.
//!
//! It opens on the startup chooser ([`boot`]): start locally, host a hub, join a hub as a node, or
//! connect to a hub. Once connected it shows the shell ([`shell`]: sidebar, top bar with STOP ALL,
//! the held banner) around one screen at a time ([`screens`]), in the design language of
//! `apps/web/DESIGN.md` ([`ui`]). Every screen talks to the hub through [`client`]. The legacy
//! register (fleet, runs, map, telemetry) stays reachable with `--legacy`.

pub mod boot;
pub mod client;
pub mod icons;
pub mod screens;
pub mod services;
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
}

fn boot() -> (Console, Task<Msg>) {
    let (boot, task) = boot::init();
    (
        Console { ctx: None, boot, shell: shell::State::default(), screens: screens::Screens::default() },
        task.map(Msg::Boot),
    )
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
    if console.ctx.is_none() {
        return boot::subscription(&console.boot).map(Msg::Boot);
    }
    Subscription::batch([
        iced::time::every(std::time::Duration::from_secs(4)).map(|_| Msg::Tick),
        shell::subscription(&console.shell).map(Msg::Shell),
    ])
}

/// Runs the console until the window closes.
pub fn run() -> iced::Result {
    iced::application(boot, update, view)
        .title(|_: &Console| "AgentPlaza".to_string())
        .theme(|_: &Console| crate::theme::theme())
        .subscription(subscription)
        .window_size([1360.0, 880.0])
        .antialiasing(true)
        .run()
}
