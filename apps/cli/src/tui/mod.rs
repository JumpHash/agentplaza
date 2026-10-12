//! `agentplaza tui`: the terminal UI — every screen, and the plaza drawn live.

pub mod floor;

use crate::ctx::Ctx;

pub fn run(ctx: &Ctx) -> Result<(), String> {
    let _ = ctx;
    Err("`agentplaza tui` is not written yet".into())
}
