//! `agentplaza node`: This machine as a node of a network: join with a code, status, leave.

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Show the state.
    Status,
}

pub fn run(cmd: Cmd, json: bool) -> Result<(), String> {
    let _ = (cmd, json);
    Err("`agentplaza node` is not written yet".into())
}
