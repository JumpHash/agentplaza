//! The `apz-workstation` binary: the command line over the library. See [`apz_workstation::cli`] for the usage.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use apz_station::AccountMode;
use apz_workstation::backend::{self, Built, Probe};
use apz_workstation::cli::{AccountArg, Command, JoinArgs, USAGE, parse};
use apz_workstation::daemon::{Daemon, DaemonConfig, Gather, host_env};
use apz_workstation::facts::{self, Extras};
use apz_workstation::join::{self, JoinRequest};
use apz_workstation::link;
use apz_workstation::log::Log;
use apz_workstation::state;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("apz-workstation {}", apz_os::version());
            ExitCode::SUCCESS
        }
        Ok(command) => match execute(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("apz-workstation: {err}");
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("apz-workstation: {err}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn execute(command: Command) -> Result<(), String> {
    match command {
        Command::Join(args) => join_command(args),
        Command::Run {
            state,
            fake_desktop,
        } => run_command(&state_dir(state)?, fake_desktop),
        Command::Status { state } => status_command(&state_dir(state)?),
        Command::Leave { state } => {
            let dir = state_dir(state)?;
            state::forget(&dir)?;
            println!(
                "this machine is no longer a workstation (its log is kept in {})",
                dir.display()
            );
            println!("the hub still lists the station until the owner removes it there");
            Ok(())
        }
        Command::Help | Command::Version => Ok(()),
    }
}

fn state_dir(given: Option<PathBuf>) -> Result<PathBuf, String> {
    match given {
        Some(dir) => Ok(dir),
        None => state::default_state_dir(),
    }
}

/// The account a join names: `agent:<user>` as given, `owner` as the current user, else the current user's
/// separate account.
fn account_for(arg: Option<AccountArg>) -> AccountMode {
    let current = current_user();
    match arg {
        Some(AccountArg::Agent(name)) => AccountMode::AgentUser { name },
        Some(AccountArg::Owner) => AccountMode::Owner { name: current },
        None => AccountMode::AgentUser { name: current },
    }
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "agent".to_string())
}

fn join_command(args: JoinArgs) -> Result<(), String> {
    let dir = state_dir(args.state.clone())?;
    let probe = Probe::here();
    let built = Built::here();
    let choice = backend::choose(&probe, built);
    let mut desktop = backend::open(choice, &probe, built, &dir);
    let account = account_for(args.account);
    let apps = backend::apps_count(choice, desktop.as_mut());
    let extras = Extras {
        account: account.clone(),
        apps,
        virtual_displays: backend::virtual_displays(&probe, built),
        ui_tree: backend::ui_tree(&probe, choice, built),
    };
    let facts = facts::gather(&extras);
    let name = args.name.clone().unwrap_or_else(|| facts.hostname.clone());
    let name = if name.is_empty() {
        "workstation".to_string()
    } else {
        name
    };
    let record = match args.from_node {
        Some(node) => join::join_from_node(&dir, &node, name, account, &facts)?,
        None => {
            let request = JoinRequest {
                hub: args.hub,
                code: args.code,
                name,
                account,
                pin: args.pin,
            };
            join::join(&dir, &request, &facts)?
        }
    };
    println!(
        "joined {} as station {} ({}); run `apz-workstation run` to connect",
        record.hub, record.station, record.name
    );
    Ok(())
}

fn run_command(dir: &Path, fake_desktop: bool) -> Result<(), String> {
    let record = state::load_record(dir)?.ok_or_else(|| {
        "this machine is not joined: run `apz-workstation join` first".to_string()
    })?;
    let probe = Probe::here();
    let built = Built::here();
    let choice = backend::select(fake_desktop || backend::fake_requested(), &probe, built);
    let desktop = backend::open(choice, &probe, built, dir);
    let account = record.account.clone();
    let virtual_displays = backend::virtual_displays(&probe, built);
    let ui_tree = backend::ui_tree(&probe, choice, built);
    let gather: Gather = Arc::new(move |apps| {
        facts::gather(&Extras {
            account: account.clone(),
            apps,
            virtual_displays,
            ui_tree,
        })
    });
    let config = DaemonConfig {
        state_dir: dir.to_path_buf(),
        env: host_env(),
        gather,
        log: Log::open(dir),
        backend: choice,
    };
    let daemon = Daemon::start(desktop, config)?;
    daemon.shared.log.info(format!(
        "apz-workstation {} starting on the {} backend",
        apz_os::version(),
        choice.name()
    ));
    match choice {
        backend::Backend::Fake => daemon.shared.log.warn(
            "--fake-desktop: screen operations run on the fake desktop, not this machine's screen",
        ),
        backend::Backend::Unavailable => daemon.shared.log.warn(format!(
            "no desktop backend: {}; screen operations are refused with this reason",
            backend::unavailable_reason(&probe, built)
        )),
        _ => {}
    }
    let stop = AtomicBool::new(false);
    link::run(&daemon, &record, &stop)
}

fn status_command(dir: &Path) -> Result<(), String> {
    let Some(record) = state::load_record(dir)? else {
        println!("not joined (no station record in {})", dir.display());
        return Ok(());
    };
    println!("station {} ({})", record.station, record.name);
    println!("hub {}", record.hub);
    println!("account {}", describe_account(&record.account));
    match state::load_status(dir)? {
        Some(status) => {
            println!(
                "link {} on the {} backend (version {}, pid {})",
                if status.connected { "up" } else { "down" },
                status.backend,
                status.version,
                status.pid
            );
            if let Some(error) = status.last_error {
                println!("last error: {error}");
            }
        }
        None => println!("link: the daemon has not run here yet"),
    }
    Ok(())
}

fn describe_account(account: &AccountMode) -> String {
    match account {
        AccountMode::AgentUser { name } => format!("agent:{name}"),
        AccountMode::Owner { name } => format!("owner ({name}): agents act as this account"),
    }
}
