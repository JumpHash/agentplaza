//! `agentplaza`: the AgentPlaza command line. Every console feature as a command (the same hub API
//! the web console and the desktop app use), `agentplaza chats` to talk to the orchestrator and the
//! planners, `agentplaza tui` for the plaza live in the terminal, and `agentplaza node` to make this
//! machine a node of a network.

mod cmd;
mod ctx;
mod rich;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "agentplaza", version = apz_os::version(), about = "AgentPlaza: a plaza for your AI agents, from the terminal", propagate_version = true)]
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
    /// The plaza at a glance: hub, nodes, agents, work.
    Overview,
    /// Sessions: the work engine's sessions (a seat's step, a project's planner, the orchestrator): list the live
    /// ones first, show one, its context and transcript, or stop it.
    Sessions(cmd::sessions::Args),
    /// A task's seats: each filled by one session at a time, with its state, model, attempts and round.
    Seats(cmd::seats::Args),
    /// The inbox: the cards waiting for you, each with its buttons; `inbox answer` gives one of them.
    Inbox(cmd::inbox::Args),
    /// Budgets: a project's and a task's (cap, spent, reserved, held, left), set with `--set`, and their ledgers.
    #[command(subcommand)]
    Budget(cmd::budget::Cmd),
    /// Chats: the orchestrator's chat (`main`), a project's chat with its planner and a task's: the list (`--project`,
    /// `--archived`), show, post, open a chat with a project, archive one.
    Chats(cmd::chats::Args),
    /// Agents: the VMs the sessions run on, by cid: talk to one, stop, restart, retire, wipe, sleep, checkpoint,
    /// roll back, fork, and set its model.
    #[command(subcommand)]
    Agents(cmd::agents::Cmd),
    /// Seat templates: the roles a task's seats are filled from, built in and custom. `roles` lists them; `save` and
    /// `delete` change the custom ones.
    Roles(cmd::roles::Args),
    /// Projects: open one, pause, resume or archive it, its settings, its plan's stages.
    #[command(subcommand)]
    Projects(cmd::projects::Cmd),
    /// Tasks: the work queue, each task in a project.
    #[command(subcommand)]
    Tasks(cmd::tasks::Cmd),
    /// Weft: the plaza's version control (history, lanes, releases, git export)
    Weft(cmd::weft::Args),
    /// Git platforms (GitHub, GitLab, Forgejo): what the plaza is authorized on and may use, and connecting a
    /// repository to a project.
    #[command(subcommand)]
    Platforms(cmd::platforms::Cmd),
    /// Review recipes
    #[command(subcommand)]
    Recipes(cmd::recipes::Cmd),
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
    /// The board: posts and announcements.
    #[command(subcommand)]
    Board(cmd::board::Cmd),
    /// Model usage: what the calls cost over a window, the series, the call log, and the prices.
    #[command(subcommand)]
    Usage(cmd::usage::Cmd),
    /// Metrics: the groups with the largest values, a metric's series, the overview as it changes, and the alerts.
    #[command(subcommand)]
    Metrics(cmd::metrics::Cmd),
    /// The plaza's laws.
    #[command(subcommand)]
    Rules(cmd::rules::Cmd),
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
    /// The network's nodes: list, join codes, stop and resume, rebalance, revoke.
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
    /// What a local plaza needs on this machine: its services beside the app, its ports, a hypervisor for the
    /// agents. No Docker or database is needed; a requirement that is not met says what to do.
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
        Command::Sessions(a) => cmd::sessions::run(&ctx, a),
        Command::Seats(a) => cmd::seats::run(&ctx, a),
        Command::Inbox(a) => cmd::inbox::run(&ctx, a),
        Command::Budget(c) => cmd::budget::run(&ctx, c),
        Command::Chats(c) => cmd::chats::run(&ctx, c),
        Command::Agents(c) => cmd::agents::run(&ctx, c),
        Command::Roles(args) => cmd::roles::run(&ctx, args),
        Command::Projects(c) => cmd::projects::run(&ctx, c),
        Command::Tasks(c) => cmd::tasks::run(&ctx, c),
        Command::Weft(args) => cmd::weft::run(&ctx, args),
        Command::Platforms(c) => cmd::platforms::run(&ctx, c),
        Command::Recipes(c) => cmd::recipes::run(&ctx, c),
        Command::Ide(c) => cmd::ide::run(&ctx, c),
        Command::Library(c) => cmd::library::run(&ctx, c),
        Command::Prompts(c) => cmd::prompts::run(&ctx, c),
        Command::Skills(c) => cmd::skills::run(&ctx, c),
        Command::Approvals(c) => cmd::approvals::run(&ctx, c),
        Command::Board(c) => cmd::board::run(&ctx, c),
        Command::Usage(c) => cmd::usage::run(&ctx, c),
        Command::Metrics(c) => cmd::metrics::run(&ctx, c),
        Command::Rules(c) => cmd::rules::run(&ctx, c),
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

    /// The work engine's commands parse as their help says: sessions with their filters and actions, seats, the inbox
    /// and its answers, the budgets and their ledgers, and the chats.
    #[test]
    fn the_work_engine_commands_parse() {
        let ok = |args: &[&str]| {
            let mut all = vec!["agentplaza"];
            all.extend_from_slice(args);
            super::Cli::try_parse_from(all).is_ok()
        };
        assert!(ok(&["sessions"]));
        assert!(ok(&["sessions", "--task", "t4", "--live"]));
        assert!(ok(&["sessions", "--state", "paused"]));
        assert!(!ok(&["sessions", "--state", "sleeping"]), "no such state");
        assert!(ok(&["sessions", "show", "s3"]));
        assert!(ok(&["sessions", "context", "s3"]));
        assert!(ok(&["sessions", "transcript", "s3", "--follow"]));
        assert!(ok(&["sessions", "stop", "s3", "--yes"]));
        assert!(!ok(&["sessions", "show"]), "show names a session");
        assert!(ok(&["seats", "t4"]));
        assert!(ok(&["inbox"]));
        assert!(ok(&["inbox", "answer", "c7", "raise", "--amount", "12.5"]));
        assert!(ok(&["inbox", "answer", "c7", "start"]));
        assert!(!ok(&["inbox", "answer", "c7"]), "answer names a choice");
        assert!(ok(&[
            "budget", "project", "pr3", "--set", "usd", "50", "--warn", "90"
        ]));
        assert!(ok(&[
            "budget",
            "project",
            "pr3",
            "--set",
            "tokens",
            "200000",
            "--monthly",
            "5000"
        ]));
        assert!(
            !ok(&["budget", "project", "pr3", "--warn", "90"]),
            "a warn share goes with --set"
        );
        assert!(
            ok(&["budget", "task", "t4", "--set", "usd"]),
            "a budget with no cap"
        );
        assert!(ok(&["budget", "ledger", "pr3", "--limit", "20"]));
        assert!(ok(&["budget", "ledger", "t4", "--task"]));
        assert!(ok(&["chats"]));
        assert!(ok(&["chats", "--project", "pr3", "--archived"]));
        assert!(ok(&["chats", "show", "main", "--limit", "20"]));
        assert!(ok(&["chats", "post", "main", "ship it"]));
        assert!(ok(&["chats", "open", "pr3", "--title", "Release 0.9"]));
        assert!(ok(&["chats", "archive", "chat-pr3", "--undo"]));
        assert!(!ok(&["chat", "--list"]), "the chat command is chats now");
    }

    /// The project-scoped flags and subcommands parse as their help says.
    #[test]
    fn the_project_flags_and_subcommands_parse() {
        let ok = |args: &[&str]| {
            let mut all = vec!["agentplaza"];
            all.extend_from_slice(args);
            super::Cli::try_parse_from(all).is_ok()
        };
        assert!(ok(&["sessions", "--project", "pr3", "--live"]));
        assert!(!ok(&["sessions", "--bench"]), "no such option");
        assert!(
            !ok(&["agents", "list"]),
            "the agents' roster is the sessions"
        );
        assert!(
            !ok(&["agents", "create", "--role", "builder"]),
            "seats are filled by sessions, not spawned by hand"
        );
        assert!(
            ok(&["agents", "stop", "7", "--yes"]),
            "a VM is still stopped by its cid"
        );
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
        assert!(!ok(&["projects", "members", "pr3"]), "no such command");
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
        // A project's life after launch: its mode, adopting one, maintenance, environments, files.
        assert!(ok(&["projects", "modes"]));
        assert!(ok(&["projects", "mode", "pr3", "maintenance"]));
        assert!(
            !ok(&["projects", "mode", "pr3", "frozen"]),
            "only the hub's modes"
        );
        assert!(ok(&[
            "projects",
            "adopt",
            "Shop",
            "--git",
            "https://example.com/shop.git",
            "--env",
            "production=production,https://shop.example.com"
        ]));
        assert!(ok(&["projects", "maintenance", "pr3"]));
        assert!(ok(&["projects", "maintenance", "pr3", "sweep"]));
        assert!(ok(&[
            "projects",
            "maintenance",
            "pr3",
            "approve",
            "mp7",
            "--note",
            "this week"
        ]));
        assert!(ok(&[
            "projects",
            "maintenance",
            "pr3",
            "decline",
            "mp8",
            "--reason",
            "not now"
        ]));
        assert!(ok(&["projects", "env", "pr3"]));
        assert!(ok(&["projects", "env", "pr3", "deploy", "staging"]));
        assert!(ok(&["projects", "upload", "pr3", "README.md"]));
    }
}
