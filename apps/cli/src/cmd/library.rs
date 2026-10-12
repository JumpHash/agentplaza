//! `agentplaza library`: The library: shared files and images.

use clap::Subcommand;

use crate::ctx::Ctx;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them.
    List,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    let _ = (ctx, cmd);
    Err("`agentplaza library` is not written yet".into())
}
