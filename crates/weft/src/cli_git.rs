//! The CLI's git, release and artifact commands.

use crate::cli::Ctx;

pub fn run(_cx: &Ctx, cmd: &[String]) -> Result<(), String> {
    Err(format!(
        "no command `{}` (weft help lists them)",
        cmd.join(" ")
    ))
}
