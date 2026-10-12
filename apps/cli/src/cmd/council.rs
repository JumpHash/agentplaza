//! `agentplaza council`: The council: motions and votes.

use clap::Subcommand;

use crate::ctx::Ctx;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them.
    List,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    let _ = (ctx, cmd);
    Err("`agentplaza council` is not written yet".into())
}
