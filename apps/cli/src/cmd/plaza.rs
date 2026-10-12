//! `agentplaza plaza`: STOP ALL and resume.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, confirm, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Whether the plaza runs or is stopped: since when, by whom, why.
    State,
    /// STOP ALL: every agent's work is cancelled and nothing new starts until `plaza resume`.
    /// Asks to confirm unless --yes.
    Stop {
        /// Why the plaza stops (kept with the stop).
        #[arg(long, default_value = "")]
        reason: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Resume the plaza after a STOP ALL.
    Resume,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::State => {
            let state = ctx.client.get("/v1/plaza/state")?;
            ctx.out(&state, print_state);
            Ok(())
        }
        Cmd::Stop { reason, yes } => {
            if !confirm(yes, "STOP ALL: every agent's work is cancelled and nothing new starts.")? {
                println!("cancelled");
                return Ok(());
            }
            let state = ctx.client.post("/v1/plaza/stop", &json!({ "reason": reason }))?;
            ctx.out(&state, |s| println!("plaza stopped; {} work item(s) cancelled", ctx::text(s, "cancelled")));
            Ok(())
        }
        Cmd::Resume => {
            let state = ctx.client.post("/v1/plaza/resume", &json!({}))?;
            ctx.out(&state, |_| println!("plaza resumed"));
            Ok(())
        }
    }
}

fn print_state(s: &Value) {
    let stopped = s["paused"].as_bool().unwrap_or(false);
    ctx::fields(&[
        ("state", if stopped { "STOPPED" } else { "running" }.to_string()),
        ("since", ctx::when(&s["since_ms"])),
        ("by", ctx::s(s, "by").to_string()),
        ("reason", ctx::s(s, "reason").to_string()),
        ("cancelled", ctx::text(s, "cancelled")),
    ]);
}
