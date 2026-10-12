//! `agentplaza`: the AgentPlaza command line. Every console feature as a command (the same hub API
//! the web console and the desktop app use), `agentplaza tui` for the plaza live in the terminal,
//! and `agentplaza node` to make this machine a node of a network.

mod cmd;
mod ctx;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "agentplaza", version = apz_os::VERSION, about = "AgentPlaza: a town for your AI agents, from the terminal", propagate_version = true)]
struct Cli {
    /// The hub: a URL or the name of a saved hub (default: the saved default, else the local hub).
    #[arg(long, global = true, env = "APZ_HUB")]
    hub: Option<String>,
    /// The owner token (default: the saved hub's, or the local hub's own).
    #[arg(long, global = true, env = "APZ_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// The hub certificate's SHA-256 pin, hex (a remote hub's own certificate).
    #[arg(long, global = true, env = "APZ_PIN")]
    pin: Option<String>,
    /// Print the hub's JSON instead of tables.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// The plaza at a glance: hub, nodes, agents, work, spend.
    Overview,
    /// Agents: list, inspect, create, talk to, stop, restart, set models.
    #[command(subcommand)]
    Agents(cmd::agents::Cmd),
    /// Projects and their repositories.
    #[command(subcommand)]
    Projects(cmd::projects::Cmd),
    /// Tasks: the work queue.
    #[command(subcommand)]
    Tasks(cmd::tasks::Cmd),
    /// Git: the plaza's repositories, branches and changes.
    #[command(subcommand)]
    Git(cmd::git::Cmd),
    /// The library: files and images the agents share.
    #[command(subcommand)]
    Library(cmd::library::Cmd),
    /// Approvals waiting for the owner.
    #[command(subcommand)]
    Approvals(cmd::approvals::Cmd),
    /// The society: roles, bonds, reputation.
    #[command(subcommand)]
    Society(cmd::society::Cmd),
    /// Conversations between agents and with the owner.
    #[command(subcommand)]
    Conversations(cmd::conversations::Cmd),
    /// The board: posts and announcements.
    #[command(subcommand)]
    Board(cmd::board::Cmd),
    /// The council: motions and votes.
    #[command(subcommand)]
    Council(cmd::council::Cmd),
    /// The treasury: budgets, balances, spend.
    #[command(subcommand)]
    Treasury(cmd::treasury::Cmd),
    /// The plaza's laws.
    #[command(subcommand)]
    Rules(cmd::rules::Cmd),
    /// Model providers, keys, policy, and decision models.
    #[command(subcommand)]
    Models(cmd::models::Cmd),
    /// Tools and connectors the agents may use.
    #[command(subcommand)]
    Tools(cmd::tools::Cmd),
    /// Machines: the VMs, their resources and runs.
    #[command(subcommand)]
    Machines(cmd::machines::Cmd),
    /// The network's nodes: list, join codes, drain, revoke.
    #[command(subcommand)]
    Nodes(cmd::nodes::Cmd),
    /// This machine as a node of a network: join with a code, status, leave.
    #[command(subcommand)]
    Node(cmd::node::Cmd),
    /// STOP ALL and resume: the plaza's master switch.
    #[command(subcommand)]
    Plaza(cmd::plaza::Cmd),
    /// Ask the plaza's decision model a question.
    Decide(cmd::decide::Args),
    /// Stream the plaza's live events (the hub's WebSocket).
    Events(cmd::events::Args),
    /// The terminal UI: every screen, and the plaza drawn live.
    Tui,
    /// Hubs: sign in to one, list and pick the saved ones.
    #[command(subcommand)]
    Hub(cmd::hub::Cmd),
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("agentplaza: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    // Saved-hub commands work on the local configuration only.
    if let Command::Hub(cmd) = cli.command {
        return cmd::hub::run(cmd, cli.json);
    }
    if let Command::Node(cmd) = cli.command {
        return cmd::node::run(cmd, cli.json);
    }
    let ctx = ctx::resolve(cli.hub, cli.token, cli.pin, cli.json)?;
    match cli.command {
        Command::Overview => cmd::overview::run(&ctx),
        Command::Agents(c) => cmd::agents::run(&ctx, c),
        Command::Projects(c) => cmd::projects::run(&ctx, c),
        Command::Tasks(c) => cmd::tasks::run(&ctx, c),
        Command::Git(c) => cmd::git::run(&ctx, c),
        Command::Library(c) => cmd::library::run(&ctx, c),
        Command::Approvals(c) => cmd::approvals::run(&ctx, c),
        Command::Society(c) => cmd::society::run(&ctx, c),
        Command::Conversations(c) => cmd::conversations::run(&ctx, c),
        Command::Board(c) => cmd::board::run(&ctx, c),
        Command::Council(c) => cmd::council::run(&ctx, c),
        Command::Treasury(c) => cmd::treasury::run(&ctx, c),
        Command::Rules(c) => cmd::rules::run(&ctx, c),
        Command::Models(c) => cmd::models::run(&ctx, c),
        Command::Tools(c) => cmd::tools::run(&ctx, c),
        Command::Machines(c) => cmd::machines::run(&ctx, c),
        Command::Nodes(c) => cmd::nodes::run(&ctx, c),
        Command::Plaza(c) => cmd::plaza::run(&ctx, c),
        Command::Decide(a) => cmd::decide::run(&ctx, a),
        Command::Events(a) => cmd::events::run(&ctx, a),
        Command::Tui => tui::run(&ctx),
        Command::Hub(_) | Command::Node(_) => unreachable!("handled above"),
    }
}
