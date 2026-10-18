//! `agentplaza`: the AgentPlaza command line. Every console feature as a command (the same hub API
//! the web console and the desktop app use), `agentplaza chat` to talk a project into being with the
//! orchestrator, `agentplaza tui` for the plaza live in the terminal, and `agentplaza node` to make
//! this machine a node of a network.

mod cmd;
mod ctx;
mod rich;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "agentplaza", version = apz_os::version(), about = "AgentPlaza: a town for your AI agents, from the terminal", propagate_version = true)]
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
    /// Print agent text as written: no short labels, colour or links (JSON is always as the hub sends it).
    #[arg(long, global = true)]
    plain: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// The plaza at a glance: hub, nodes, agents, work, spend.
    Overview,
    /// Talk to the orchestrator: a conversation drafts a project (its tasks, team, budget and settings), and /start
    /// opens, staffs and plans it at once. `--list` lists your conversations.
    Chat(cmd::chat::Args),
    /// Agents: list (all, a project's, or the bench), inspect, create, talk to, stop, restart, set models.
    #[command(subcommand)]
    Agents(cmd::agents::Cmd),
    /// Projects: open one, put agents on it, pause, resume or archive it, its settings and budget, its plan's stages.
    #[command(subcommand)]
    Projects(cmd::projects::Cmd),
    /// Tasks: the work queue, each task in a project.
    #[command(subcommand)]
    Tasks(cmd::tasks::Cmd),
    /// Git: the plaza's repositories, branches and changes.
    #[command(subcommand)]
    Git(cmd::git::Cmd),
    /// The IDE: an agent's workspace or a project repo, browsed, edited, diffed, checkpointed, searched and run.
    #[command(subcommand)]
    Ide(cmd::ide::Cmd),
    /// The library: files and images the agents share.
    #[command(subcommand)]
    Library(cmd::library::Cmd),
    /// The prompt library: system prompts, AGENTS.md and fragments, versioned, checked, published and bound.
    #[command(subcommand)]
    Prompts(cmd::prompts::Cmd),
    /// The skill library: SKILL.md folders, versioned, checked, bound, imported from zips or GitHub, and exported.
    #[command(subcommand)]
    Skills(cmd::skills::Cmd),
    /// Approvals waiting for the owner.
    #[command(subcommand)]
    Approvals(cmd::approvals::Cmd),
    /// The society: roles, bonds, reputation.
    #[command(subcommand)]
    Society(cmd::society::Cmd),
    /// Conversations between agents (yours with the orchestrator are `chat`).
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
    /// Model usage: what the calls cost over a window, the series, the call log, and the prices.
    #[command(subcommand)]
    Usage(cmd::usage::Cmd),
    /// Metrics: the groups with the largest values, a metric's series, the overview as it changes, and the alerts.
    #[command(subcommand)]
    Metrics(cmd::metrics::Cmd),
    /// The plaza's laws.
    #[command(subcommand)]
    Rules(cmd::rules::Cmd),
    /// Sanctions: what the laws' violations drew, and lifting a tool suspension.
    #[command(subcommand)]
    Sanctions(cmd::sanctions::Cmd),
    /// The vault: the hub's encrypted secrets, scoped to nodes and bound to what uses them.
    #[command(subcommand)]
    Vault(cmd::vault::Cmd),
    /// Model providers, keys, policy, and decision models.
    #[command(subcommand)]
    Models(cmd::models::Cmd),
    /// The training lab: devices, jobs, recipes, datasets, models, Hugging Face imports, and the breeders.
    #[command(subcommand)]
    Lab(cmd::lab::Cmd),
    /// Tools and connectors the agents may use.
    #[command(subcommand)]
    Tools(cmd::tools::Cmd),
    /// Machines: the VMs, their resources and runs.
    #[command(subcommand)]
    Machines(cmd::machines::Cmd),
    /// The network's nodes: list, join codes, drain, revoke.
    #[command(subcommand)]
    Nodes(cmd::nodes::Cmd),
    /// Workstations on the hub: list, join codes, leases, settings with the takeover policy, drain, revoke, audit.
    #[command(subcommand)]
    Stations(cmd::stations::Cmd),
    /// This machine as a node of a network: join with a code, status, leave.
    #[command(subcommand)]
    Node(cmd::node::Cmd),
    /// This machine as a workstation the agents work through the hub: join with a code (and its account and service),
    /// status, leave.
    #[command(subcommand)]
    Workstation(cmd::workstation::Cmd),
    /// What a local plaza needs on this machine (Docker, Postgres, Forgejo, Garage, the ports): check it,
    /// and fix the missing parts after you confirm what the fix runs.
    Setup(cmd::setup::Args),
    /// STOP ALL and resume: the plaza's master switch.
    #[command(subcommand)]
    Plaza(cmd::plaza::Cmd),
    /// The hub's card for a link or a reference (a task `#N`, an agent `@Name`): what the hover card shows.
    Unfurl(cmd::unfurl::Args),
    /// Ask the plaza's decision model a question.
    Decide(cmd::decide::Args),
    /// Stream the plaza's live events (the hub's WebSocket).
    Events(cmd::events::Args),
    /// The terminal UI: every screen, the plaza drawn live, a project switcher and the chat with the orchestrator (a
    /// setup and a tour on first start).
    Tui(tui::Args),
    /// Hubs: sign in to one, list and pick the saved ones.
    #[command(subcommand)]
    Hub(cmd::hub::Cmd),
    /// The hub as an MCP server: turn it on, make tokens, and the stdio bridge for MCP clients.
    #[command(subcommand)]
    Mcp(cmd::mcp::Cmd),
    /// The stdio agent editors start (Zed, JetBrains, Neovim); `acp sessions` lists the editor sessions on the hub.
    Acp(cmd::acp::Args),
    /// Egress credentials: rules that add a vault secret to an agent's outbound HTTPS request, so the secret never enters the VM.
    #[command(subcommand)]
    Egress(cmd::egress::Cmd),
    /// The hub's E2B-compatible sandbox API: its keys for the E2B SDKs, and the sandboxes on the hub.
    #[command(subcommand)]
    E2b(cmd::e2b::Cmd),
    /// The hub machine's warm templates: new agents and sandboxes restore one instead of booting.
    #[command(subcommand)]
    Templates(cmd::templates::Cmd),
    /// The hub's shared read-only data volumes, which each role's agents mount when spawned.
    #[command(subcommand)]
    Volumes(cmd::volumes::Cmd),
}

fn main() {
    // Like any Unix tool: when the reader goes away (`agentplaza events | head`), end quietly
    // instead of panicking on a broken pipe.
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of SIGPIPE before any thread starts.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("agentplaza: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    // Saved-hub commands work on the local configuration only.
    if let Command::Hub(cmd) = cli.command {
        return cmd::hub::run(cmd, cli.json, cli.hub, cli.token, cli.pin);
    }
    if let Command::Node(cmd) = cli.command {
        return cmd::node::run(cmd, cli.json);
    }
    // This machine as a workstation: the hub's address and pin come from the global flags, not a saved hub.
    if let Command::Workstation(cmd) = cli.command {
        return cmd::workstation::run(cmd, cli.json, cli.hub, cli.pin);
    }
    // This machine's own setup; `--hub` also checks the hub named by the global flags.
    if let Command::Setup(args) = cli.command {
        return cmd::setup::run(args, cli.json, cli.hub, cli.token, cli.pin);
    }
    if let Command::Mcp(cmd) = cli.command {
        let globals = cmd::mcp::Globals {
            hub: cli.hub,
            token: cli.token,
            pin: cli.pin,
            json: cli.json,
        };
        return cmd::mcp::run(cmd, globals);
    }
    if let Command::Acp(args) = cli.command {
        return cmd::acp::run(args, cli.hub, cli.token, cli.pin, cli.json);
    }
    let terminal = rich::Terminal::from_env(cli.plain);
    let mut ctx = ctx::resolve(cli.hub, cli.token, cli.pin, cli.json)?;
    ctx.rich = rich::Mode::decide(&terminal, Some(ctx.client.base()));
    match cli.command {
        Command::Overview => cmd::overview::run(&ctx),
        Command::Chat(a) => cmd::chat::run(&ctx, a),
        Command::Agents(c) => cmd::agents::run(&ctx, c),
        Command::Projects(c) => cmd::projects::run(&ctx, c),
        Command::Tasks(c) => cmd::tasks::run(&ctx, c),
        Command::Git(c) => cmd::git::run(&ctx, c),
        Command::Ide(c) => cmd::ide::run(&ctx, c),
        Command::Library(c) => cmd::library::run(&ctx, c),
        Command::Prompts(c) => cmd::prompts::run(&ctx, c),
        Command::Skills(c) => cmd::skills::run(&ctx, c),
        Command::Approvals(c) => cmd::approvals::run(&ctx, c),
        Command::Society(c) => cmd::society::run(&ctx, c),
        Command::Conversations(c) => cmd::conversations::run(&ctx, c),
        Command::Board(c) => cmd::board::run(&ctx, c),
        Command::Council(c) => cmd::council::run(&ctx, c),
        Command::Treasury(c) => cmd::treasury::run(&ctx, c),
        Command::Usage(c) => cmd::usage::run(&ctx, c),
        Command::Metrics(c) => cmd::metrics::run(&ctx, c),
        Command::Rules(c) => cmd::rules::run(&ctx, c),
        Command::Sanctions(c) => cmd::sanctions::run(&ctx, c),
        Command::Vault(c) => cmd::vault::run(&ctx, c),
        Command::Models(c) => cmd::models::run(&ctx, c),
        Command::Lab(c) => cmd::lab::run(&ctx, c),
        Command::Tools(c) => cmd::tools::run(&ctx, c),
        Command::Machines(c) => cmd::machines::run(&ctx, c),
        Command::Nodes(c) => cmd::nodes::run(&ctx, c),
        Command::Stations(c) => cmd::stations::run(&ctx, c),
        Command::Egress(c) => cmd::egress::run(&ctx, c),
        Command::E2b(c) => cmd::e2b::run(&ctx, c),
        Command::Templates(c) => cmd::templates::run(&ctx, c),
        Command::Volumes(c) => cmd::volumes::run(&ctx, c),
        Command::Plaza(c) => cmd::plaza::run(&ctx, c),
        Command::Unfurl(a) => cmd::unfurl::run(&ctx, a),
        Command::Decide(a) => cmd::decide::run(&ctx, a),
        Command::Events(a) => cmd::events::run(&ctx, a),
        Command::Tui(args) => tui::run(&ctx, args),
        Command::Hub(_)
        | Command::Node(_)
        | Command::Workstation(_)
        | Command::Mcp(_)
        | Command::Acp(_)
        | Command::Setup(_) => {
            unreachable!("handled above")
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    /// Clap checks a command definition (clashing flags, bad defaults) only in debug builds; CI tests in release, so
    /// this asks for the check on the whole command tree, every subcommand included.
    #[test]
    fn the_command_tree_is_valid() {
        super::Cli::command().debug_assert();
    }

    /// The new command groups are reachable under their names: `egress` and `e2b`, with `e2b keys` below.
    #[test]
    fn egress_and_e2b_are_registered_under_their_names() {
        let cli = super::Cli::try_parse_from(["agentplaza", "egress", "list"]).unwrap();
        assert!(matches!(cli.command, super::Command::Egress(_)));
        let cli = super::Cli::try_parse_from(["agentplaza", "e2b", "keys", "list"]).unwrap();
        assert!(matches!(cli.command, super::Command::E2b(_)));
        assert!(super::Cli::try_parse_from(["agentplaza", "e2b"]).is_err());
    }

    /// `chat` takes its message as words, and goes on with a conversation by its id; `--list` stands alone.
    #[test]
    fn chat_takes_a_message_and_a_conversation() {
        let cli = super::Cli::try_parse_from([
            "agentplaza",
            "chat",
            "--conversation",
            "conv-3",
            "ship",
            "0.9",
        ])
        .unwrap();
        let super::Command::Chat(args) = cli.command else {
            panic!("chat parses as chat");
        };
        assert_eq!(args.conversation.as_deref(), Some("conv-3"));
        assert_eq!(args.message, ["ship", "0.9"]);
        assert_eq!(args.wait, 600);
        assert!(super::Cli::try_parse_from(["agentplaza", "chat", "--list"]).is_ok());
        assert!(
            super::Cli::try_parse_from(["agentplaza", "chat", "--list", "hello"]).is_err(),
            "--list takes no message"
        );
        assert!(
            super::Cli::try_parse_from(["agentplaza", "chat", "--archived"]).is_err(),
            "--archived goes with --list"
        );
    }

    /// The project-scoped flags and subcommands parse as their help says.
    #[test]
    fn the_project_flags_and_subcommands_parse() {
        let ok = |args: &[&str]| {
            let mut all = vec!["agentplaza"];
            all.extend_from_slice(args);
            super::Cli::try_parse_from(all).is_ok()
        };
        assert!(ok(&["agents", "list", "--project", "pr3"]));
        assert!(ok(&["agents", "list", "--bench"]));
        assert!(!ok(&["agents", "list", "--bench", "--project", "pr3"]));
        assert!(ok(&[
            "agents",
            "create",
            "--role",
            "builder",
            "--project",
            "pr3"
        ]));
        assert!(ok(&["tasks", "list", "--project", "pr3"]));
        assert!(ok(&["tasks", "create", "Fix it", "--project", "pr3"]));
        assert!(ok(&["tasks", "move", "t4", "--project", "pr3"]));
        assert!(ok(&[
            "projects",
            "create",
            "Release 0.9",
            "--goal",
            "Tag it",
            "--kind",
            "code"
        ]));
        assert!(!ok(&[
            "projects",
            "create",
            "Release 0.9",
            "--goal",
            "Tag it",
            "--kind",
            "film"
        ]));
        assert!(ok(&["projects", "members", "pr3"]));
        assert!(ok(&["projects", "members", "pr3", "add", "7"]));
        assert!(ok(&["projects", "members", "pr3", "remove", "7", "--yes"]));
        assert!(ok(&["projects", "members", "pr3", "suggested"]));
        assert!(ok(&["projects", "pause", "pr3"]));
        assert!(ok(&["projects", "resume", "pr3"]));
        assert!(ok(&["projects", "archive", "pr3", "--yes"]));
        assert!(ok(&["projects", "settings", "pr3"]));
        assert!(ok(&["projects", "settings", "pr3", "effective"]));
        assert!(ok(&[
            "projects",
            "settings",
            "pr3",
            "set",
            "effort=high",
            "max_turns=40"
        ]));
        assert!(
            !ok(&["projects", "settings", "pr3", "set"]),
            "set needs a pair"
        );
        assert!(ok(&["projects", "settings", "pr3", "unset", "effort"]));
        assert!(ok(&["projects", "budget", "pr3"]));
        assert!(ok(&["projects", "budget", "pr3", "add", "5000"]));
        assert!(ok(&["projects", "budget", "pr3", "return", "1000"]));
        assert!(
            !ok(&["projects", "budget", "pr3", "add", "0"]),
            "credits are at least 1"
        );
    }
}
