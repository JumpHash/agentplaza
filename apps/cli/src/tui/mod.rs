//! `agentplaza tui`: the terminal UI — the plaza drawn live on the floor, and every console area as
//! a tab, fed by the hub's endpoints and its live events.

pub mod floor;

mod app;
mod screens;

use crate::ctx::Ctx;

pub fn run(ctx: &Ctx) -> Result<(), String> {
    if let Some(path) = std::env::var_os("APZ_TUI_SNAPSHOT") {
        return app::snapshot(ctx.client.clone(), std::path::Path::new(&path), 160, 46, 4.0);
    }
    app::run(ctx.client.clone())
}
