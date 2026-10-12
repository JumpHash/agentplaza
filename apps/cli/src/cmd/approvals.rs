//! `agentplaza approvals`: Approvals waiting for the owner.

use clap::Subcommand;

use crate::ctx::Ctx;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them.
    List,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    let _ = (ctx, cmd);
    Err("`agentplaza approvals` is not written yet".into())
}
