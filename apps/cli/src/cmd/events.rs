//! `agentplaza events`: Stream the plaza's live events.

use crate::ctx::Ctx;

#[derive(clap::Args, Debug)]
pub struct Args {}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let _ = (ctx, args);
    Err("`agentplaza events` is not written yet".into())
}
