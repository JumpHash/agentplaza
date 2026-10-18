//! The CLI's commands for landing, git, releases and artifacts.

use crate::cli::Ctx;

pub fn run(_cx: &Ctx, cmd: &[String]) -> Result<(), String> {
    Err(format!(
        "weft: no command `{}` (weft help lists them)",
        cmd.join(" ")
    ))
}
