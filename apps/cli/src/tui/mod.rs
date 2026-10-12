//! `agentplaza tui`: the terminal UI — the plaza drawn live on the floor, and every console area as
//! a tab, fed by the hub's endpoints and its live events.

pub mod floor;

mod app;
mod screens;

use crate::ctx::Ctx;

pub fn run(ctx: &Ctx) -> Result<(), String> {
    app::run(ctx.client.clone())
}
