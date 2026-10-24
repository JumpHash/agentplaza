//! `agentplaza tui`: the terminal UI — every console area as a tab, fed by the hub's endpoints and
//! its live events. `p` switches the project the Sessions and Tasks tabs show (see `scope`), the
//! Inbox tab answers the cards waiting for you, and the Chat tab writes to your chats. The first start shows a short setup and a tour (see
//! `onboarding`); `--setup` and `--tour` show them again.

pub mod context_bar;

mod app;
mod cmdk;
mod onboarding;
mod record;
mod scope;
mod screens;
mod style;
mod workspace;
mod workspace_view;

use crate::ctx::{Config, Ctx};
use onboarding::Stage;

/// `agentplaza tui` options.
#[derive(clap::Args, Debug, Default)]
pub struct Args {
    /// Run the setup again (then the tour question).
    #[arg(long)]
    pub setup: bool,
    /// Start the tour now.
    #[arg(long)]
    pub tour: bool,
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    if let Some(path) = std::env::var_os("APZ_TUI_RECORD") {
        return record::run(ctx.client.clone(), std::path::Path::new(&path));
    }
    if let Some(path) = std::env::var_os("APZ_TUI_SNAPSHOT") {
        return app::snapshot(
            ctx.client.clone(),
            std::path::Path::new(&path),
            160,
            46,
            4.0,
        );
    }
    let start = if args.tour {
        Some(Stage::Tour(0))
    } else if args.setup || !Config::load().setup_done {
        Some(Stage::Setup(0))
    } else {
        None
    };
    app::run(ctx.client.clone(), start)
}
