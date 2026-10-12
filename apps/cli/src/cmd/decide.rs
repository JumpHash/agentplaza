//! `agentplaza decide`: Ask the plaza's decision model a question.

use crate::ctx::Ctx;

#[derive(clap::Args, Debug)]
pub struct Args {}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let _ = (ctx, args);
    Err("`agentplaza decide` is not written yet".into())
}
