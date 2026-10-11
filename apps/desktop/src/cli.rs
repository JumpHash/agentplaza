//! The command line: which daemon to read, and how long a call may take.
//!
//! `--url` and `APZ_DESKTOP_URL` both name the daemon; an argument wins over
//! the environment, and both default to `http://127.0.0.1:8899`. Only a
//! loopback host is accepted, and only an exact one — see
//! [`Endpoint::from_url`](crate::api::Endpoint::from_url).
//!
//! `--hub-url` and `APZ_DESKTOP_HUB` name the hub's read-only status surface,
//! when the host has one: a second loopback process (`vsockd --status-port`)
//! the roster reads to learn who is connected. It is off by default, and held
//! to the same exact-loopback policy with its port required — see
//! [`Endpoint::hub_url`](crate::api::Endpoint::hub_url).
//!
//! `--hub-admin-url` and `APZ_DESKTOP_HUB_ADMIN` name the hub's *admin*
//! surface (`vsockd --admin-port`), the one the operator's controls drive: the
//! roster can ask a connected agent to run one tool (`POST /tell`) or to end
//! its session (`POST /stop`). It is off by default — with no admin url the
//! controls are absent from the roster — and held to the same exact-loopback,
//! port-required policy as the status surface, for the same reason: the
//! surface is off unless its flag asked for it — see
//! [`Endpoint::hub_admin_url`](crate::api::Endpoint::hub_admin_url).

use std::time::Duration;

use crate::api::{DEFAULT_TIMEOUT, Endpoint};
use crate::model::{DEFAULT_REFRESH_MS, MAX_REFRESH_MS, MIN_REFRESH_MS};

/// The environment variable naming the daemon.
pub const URL_ENV: &str = "APZ_DESKTOP_URL";

/// The environment variable naming the hub's status surface, when there is
/// one: `vsockd --status-port`'s read-only `/status`.
pub const HUB_ENV: &str = "APZ_DESKTOP_HUB";

/// The environment variable naming the hub's admin surface, when there is one:
/// `vsockd --admin-port`'s `/tell` and `/stop`, the surface the operator's
/// controls drive.
pub const HUB_ADMIN_ENV: &str = "APZ_DESKTOP_HUB_ADMIN";

/// The environment variable bounding one call, in seconds.
pub const TIMEOUT_ENV: &str = "APZ_DESKTOP_TIMEOUT_SECS";

/// The environment variable setting the poll cadence, in milliseconds.
pub const REFRESH_ENV: &str = "APZ_DESKTOP_REFRESH_MS";

/// The longest `--timeout-secs` may be.
pub const MAX_TIMEOUT_SECS: u64 = 600;

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Print the usage and exit.
    Help,
    /// Run the dashboard.
    Run(Config),
}

/// Where the dashboard reads from, how often, and how long it waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The daemon.
    pub endpoint: Endpoint,
    /// The hub's status surface, when one was named: the read-only second
    /// port that says who is connected, rather than the register's word for
    /// who should be. `None` — the default — is a console with no hub half.
    pub hub: Option<Endpoint>,
    /// The hub's admin surface, when one was named: the second port the
    /// operator's controls drive (`POST /tell`, `POST /stop`). `None` — the
    /// default — is a console with no operator controls.
    pub hub_admin: Option<Endpoint>,
    /// The bound on one call.
    pub timeout: Duration,
    /// How often a healthy daemon is polled, in milliseconds.
    pub refresh_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint: Endpoint::default(),
            hub: None,
            hub_admin: None,
            timeout: DEFAULT_TIMEOUT,
            refresh_ms: DEFAULT_REFRESH_MS,
        }
    }
}

/// The usage text.
pub const HELP: &str = "\
agentplaza-desktop — the AgentPlaza fleet register

USAGE:
    agentplaza-desktop [--url <url>] [--hub-url <host:port>]
                       [--hub-admin-url <host:port>]
                       [--refresh-ms <milliseconds>] [--timeout-secs <seconds>]

OPTIONS:
    --url <url>            the node daemon to read; http://127.0.0.1:8899 by
                           default. Only a loopback host is accepted.
    --hub-url <host:port>  the hub's read-only status surface
                           (vsockd --status-port), whose GET /status says who
                           is connected right now. Only a loopback host is
                           accepted, and the port is required. Off by default:
                           without it the roster draws the register's half
                           only.
    --hub-admin-url <host:port>
                           the hub's admin surface (vsockd --admin-port), which
                           the roster's operator controls drive: one row's
                           'drive' asks a connected agent to run one tool
                           (POST /tell) and its 'stop' asks it to end its
                           session (POST /stop). Only a loopback host is
                           accepted, and the port is required. Off by default:
                           without it the controls are absent from the roster.
    --refresh-ms <n>       how often a healthy daemon is polled, 250–30000;
                           2000 by default. A failing daemon backs off from
                           here, doubling to a 30 s ceiling.
    --timeout-secs <n>     how long one call may take, 1–600; 5 by default
    -h, --help             print this help

ENVIRONMENT:
    APZ_DESKTOP_URL           the same as --url
    APZ_DESKTOP_HUB           the same as --hub-url
    APZ_DESKTOP_HUB_ADMIN     the same as --hub-admin-url
    APZ_DESKTOP_REFRESH_MS    the same as --refresh-ms
    APZ_DESKTOP_TIMEOUT_SECS  the same as --timeout-secs

KEYS:
    r                      the fleet register
    m                      the machine (GET /api/v0/machine)
    f                      the fleet map
    w                      the roster: who is running by role, and — when a
                           hub url is configured — who is connected to it,
                           with the operator controls when an admin url is
                           too: drive one tool, or stop a session
    l                      the ledger: the plaza's tasks, with the claim,
                           finish and withdrawal the daemon accepts on them
    y                      the treasury: the accounts, their totals and the
                           double-entry journal — read-only
    u                      the market: the calls the plaza's roled tasks
                           offer, their bids and their awards — read-only
    i                      the floor: the plaza's agents as robots — a station
                           pad per role, a ground status ring, a talking pair
                           walking adjacent, and the board's posts — read-only
    j                      the board: the plaza's public posts, newest first,
                           with the task each links and who has read it —
                           read-only
    k                      the talk: the plaza's conversation log, newest
                           first — a pair as two identities, the words as the
                           agent's own — with a filter over the pairs and one
                           pair's thread when it is narrowed — read-only
    t                      the session's telemetry, one observation per poll
    ?                      the help sheet (Escape or ? closes it)
    Escape                 back to the register (and closes the floor's
                           inspector, or widens the talk pane, when either is
                           open)
    up / down              walk the console's command history; on the floor
                           they cycle the robots, and on the talk pane they
                           walk the pairs filter
    Enter                  open the picked robot's inspector on the floor, or
                           narrow the talk pane to its picked pair (again
                           widens)

The dashboard polls GET /api/v0/runs and GET /api/v0/machine, follows
GET /api/v0/runs/{id} for the selected run, writes to a VM's console with
POST /api/v0/runs/{id}/input, spawns runs with POST /api/v0/runs and kills
them with DELETE /api/v0/runs/{id}. Between polls it follows
GET /api/v0/events — the daemon's event stream: run and VM state changes,
each VM's serial deltas and every task transition — and reconnects to it with
a bounded backoff. The polls stay the source of truth; the stream only moves
the same state sooner.
The ledger pane reads GET /api/v0/tasks while it is up and moves a task with
POST /api/v0/tasks (file), POST /api/v0/tasks/{id}/claim (as a live agent's
guest CID), POST /api/v0/tasks/{id}/finish (done or failed, with a note) and
DELETE /api/v0/tasks/{id} (withdraw); a task event for a task the ledger has
not read asks for the ledger rather than inventing a row.
The treasury pane (read-only) reads GET /api/v0/treasury and
GET /api/v0/treasury/journal together while it is up, on its own schedule: the
accounts with their derived balances, the totals line, the daemon's own note on
real spend, and every journal entry with its postings ruled as the debit and
credit sides. It writes nothing — every entry is the daemon's own. A treasury
event for an entry past the newest the pane holds reads the journal again
rather than assembling an entry from a frame.
The market pane (read-only) reads GET /api/v0/agora while it is up, on its own
schedule: every call a roled task offers, with its capability, its ceiling in
credits and the dollars they stand for, its state stamp, the bids on it and
the award that closed it (the contractor, the price and the treasury entry
that funded the call). It writes nothing — a bid and an award are made by the
daemon's own surface (POST /api/v0/tasks/{id}/bid and .../award). An agora
event for a bid or an award the board does not hold reads the board again
rather than assembling a call from a frame.
The floor pane (read-only) reads GET /api/v0/conversations and
GET /api/v0/board together while it is up, on its own schedule, and draws the
plaza in the register's own canvas with 3D geometry: a perspective-projected
ground plane through a fixed RTS camera (55° tilt, 20° yaw), a station pad per
role (the role's index in the register's order), one robot per roster agent
with a ground status ring (working / talking / idle / unreachable), and a
talking pair walking adjacent on an eased, bounded (700 ms) clock. Every
position derives from the role order and a conversation's activity — it is a
diagram of real state. The arrows cycle the robots and Enter opens a picked
robot's inspector (its role, CID, state, its contacts from
GET /api/v0/agents/{cid}/contacts, its last board post, and the thread
GET /api/v0/conversations/{a}/{b} with a pressed contact). It writes nothing.
A conversation frame for an exchange the floor does not hold reads it again;
a board frame for a post it already holds is a no-op.
The board pane (read-only) reads GET /api/v0/board while it is up, on its own
schedule: every post, newest first, with its author and role, the task it links
and 'read by N of M' from the daemon's per-agent unread counts.
The talk pane (read-only) reads GET /api/v0/conversations while it is up, on
its own schedule: every exchange, newest first, as two identity-and-role chips,
the words as the agent's own, the delivery mark (delivered, or the hub's own
reason verbatim when it did not reach, or a dash when the node said neither),
and the stamp. Its pairs filter walks the daemon's own pair summaries -- the
active mark is the floor's walk, within the daemon's exported window -- and
Enter narrows to the picked pair's thread (GET /api/v0/conversations/{a}/{b}),
Escape or Enter again widening. It writes nothing. A conversation frame for an
exchange the pane does not hold reads the log again rather than inventing one.
When --hub-url names a status surface, GET /status is read at the same
cadence and the roster joins it to the register by guest CID: an agent the
register reports and the hub does not is drawn 'registered, not connected',
and one the hub reports and the register does not is 'connected, not
registered'. The hub is a second process: its failures are its own and never
mark the daemon down.
When --hub-admin-url names the admin surface, each roster row gains the
operator's controls: 'drive' opens a console that asks the agent connected as
that row's guest CID to run one tool (POST /tell, with the tool's name, its
arguments as JSON, and a timeout), the answer or the hub's own refusal drawn in
a bounded, ruled text region; 'stop' arms, and a second press asks the agent to
end its session (POST /stop), the outcome drawn honestly — a session that ended
and its call count, or the hub's own 'not stopped' reason and nothing torn
down. With no admin url the controls are absent and the roster is read-only.
The spawn form's quick boots fill the form; the run pane exports a transcript,
and the telemetry pane the session's CSV, to the path in its box with plain
std::fs.
";

/// Parses the arguments, with `env` standing in for the process environment.
pub fn parse<I, F>(args: I, env: &F) -> Result<Invocation, String>
where
    I: IntoIterator<Item = String>,
    F: Fn(&str) -> Option<String>,
{
    let mut url = env(URL_ENV);
    let mut hub = env(HUB_ENV);
    let mut hub_admin = env(HUB_ADMIN_ENV);
    let mut timeout_secs = env(TIMEOUT_ENV).map(|value| parse_secs(&value)).transpose()?;
    let mut refresh_ms = env(REFRESH_ENV)
        .map(|value| parse_refresh_ms(&value))
        .transpose()?;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(Invocation::Help);
        }
        if let Some(value) = value_of(&arg, "--url", &mut args)? {
            url = Some(value);
            continue;
        }
        if let Some(value) = value_of(&arg, "--hub-url", &mut args)? {
            hub = Some(value);
            continue;
        }
        if let Some(value) = value_of(&arg, "--hub-admin-url", &mut args)? {
            hub_admin = Some(value);
            continue;
        }
        if let Some(value) = value_of(&arg, "--timeout-secs", &mut args)? {
            timeout_secs = Some(parse_secs(&value)?);
            continue;
        }
        if let Some(value) = value_of(&arg, "--refresh-ms", &mut args)? {
            refresh_ms = Some(parse_refresh_ms(&value)?);
            continue;
        }
        return Err(format!("unknown argument {arg:?}"));
    }

    let endpoint = match url {
        Some(url) => Endpoint::from_url(&url).map_err(|err| err.to_string())?,
        None => Endpoint::default(),
    };
    // The hub is optional and, when named, held to the same loopback-only
    // policy — with its port required, since the status surface has none.
    let hub = match hub {
        Some(url) => Some(Endpoint::hub_url(&url).map_err(|err| err.to_string())?),
        None => None,
    };
    // The hub's admin surface is optional and independent of the status
    // surface: the same loopback-only, port-required policy, and `None` (the
    // default) leaves the roster without its operator controls.
    let hub_admin = match hub_admin {
        Some(url) => Some(Endpoint::hub_admin_url(&url).map_err(|err| err.to_string())?),
        None => None,
    };
    let timeout = match timeout_secs {
        Some(secs) => Duration::from_secs(secs),
        None => DEFAULT_TIMEOUT,
    };
    Ok(Invocation::Run(Config {
        endpoint,
        hub,
        hub_admin,
        timeout,
        refresh_ms: refresh_ms.unwrap_or(DEFAULT_REFRESH_MS),
    }))
}

/// The value of `--name value` or `--name=value`, if `arg` is that option.
fn value_of<I>(arg: &str, name: &str, args: &mut I) -> Result<Option<String>, String>
where
    I: Iterator<Item = String>,
{
    if arg == name {
        return match args.next() {
            Some(value) => Ok(Some(value)),
            None => Err(format!("{name} needs a value")),
        };
    }
    if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
        if value.is_empty() {
            return Err(format!("{name} needs a value"));
        }
        return Ok(Some(value.to_string()));
    }
    Ok(None)
}

/// A whole number of seconds inside the contract's range.
fn parse_secs(value: &str) -> Result<u64, String> {
    let secs = value
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("{value:?} is not a whole number of seconds"))?;
    if !(1..=MAX_TIMEOUT_SECS).contains(&secs) {
        return Err(format!(
            "a timeout must be between 1 and {MAX_TIMEOUT_SECS} seconds"
        ));
    }
    Ok(secs)
}

/// A whole number of milliseconds inside the cadence's range.
fn parse_refresh_ms(value: &str) -> Result<u64, String> {
    let ms = value
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("{value:?} is not a whole number of milliseconds"))?;
    if !(MIN_REFRESH_MS..=MAX_REFRESH_MS).contains(&ms) {
        return Err(format!(
            "a refresh must be between {MIN_REFRESH_MS} and {MAX_REFRESH_MS} milliseconds"
        ));
    }
    Ok(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_name: &str) -> Option<String> {
        None
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn config(values: &[&str]) -> Config {
        match parse(args(values), &no_env).expect("the arguments should parse") {
            Invocation::Run(config) => config,
            Invocation::Help => panic!("not help"),
        }
    }

    #[test]
    fn no_arguments_means_the_loopbacks_daemon() {
        let config = config(&[]);
        assert_eq!(config.endpoint.url(), "http://127.0.0.1:8899");
        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
        assert_eq!(config.refresh_ms, DEFAULT_REFRESH_MS);
    }

    #[test]
    fn the_refresh_argument_takes_both_spellings_and_the_bounds() {
        assert_eq!(config(&["--refresh-ms", "500"]).refresh_ms, 500);
        assert_eq!(config(&["--refresh-ms=1000"]).refresh_ms, 1_000);
        assert_eq!(config(&["--refresh-ms", "250"]).refresh_ms, MIN_REFRESH_MS);
        assert_eq!(
            config(&["--refresh-ms", "30000"]).refresh_ms,
            MAX_REFRESH_MS
        );
        for value in ["0", "249", "30001", "-1", "soon", ""] {
            let error = parse(args(&["--refresh-ms", value]), &no_env)
                .expect_err("must be refused");
            assert!(!error.is_empty(), "{value}");
        }
        assert_eq!(
            parse(args(&["--refresh-ms"]), &no_env).expect_err("needs a value"),
            "--refresh-ms needs a value"
        );
    }

    #[test]
    fn the_refresh_environment_names_the_cadence_and_an_argument_wins() {
        let env = |name: &str| match name {
            REFRESH_ENV => Some("750".to_string()),
            _ => None,
        };
        let Invocation::Run(config) = parse(args(&[]), &env).expect("parses") else {
            panic!("not help")
        };
        assert_eq!(config.refresh_ms, 750);

        let Invocation::Run(config) =
            parse(args(&["--refresh-ms", "1500"]), &env).expect("parses")
        else {
            panic!("not help")
        };
        assert_eq!(config.refresh_ms, 1_500);

        let broken = |name: &str| match name {
            REFRESH_ENV => Some("fast".to_string()),
            _ => None,
        };
        assert!(parse(args(&[]), &broken).is_err());
    }

    #[test]
    fn the_help_documents_the_cadence_and_the_keys() {
        assert!(HELP.contains("--refresh-ms"));
        assert!(HELP.contains("250–30000"));
        for key in ["r ", "m ", "f ", "t ", "? ", "Escape"] {
            assert!(HELP.contains(key), "the help should document {key:?}");
        }
        // The roster's own key, named with what it answers.
        assert!(HELP.contains("w                      the roster: who is running by role"));
        // The ledger's and the treasury's own keys.
        assert!(HELP.contains("l                      the ledger"));
        assert!(HELP.contains("y                      the treasury"));
        // The market's key, and that it is read-only and reads the board.
        assert!(HELP.contains("u                      the market"));
        assert!(HELP.contains("GET /api/v0/agora"));
        // The floor's and the board's keys, and what the floor reads and
        // draws.
        assert!(HELP.contains("i                      the floor"));
        assert!(HELP.contains("j                      the board"));
        assert!(HELP.contains("k                      the talk"));
        assert!(HELP.contains("GET /api/v0/conversations"));
        assert!(HELP.contains("55° tilt, 20° yaw"));
        assert!(HELP.contains("Enter                  open the picked robot's inspector"));
        // The treasury pane is read-only and reads both halves at once.
        assert!(HELP.contains("GET /api/v0/treasury/journal"));
        assert!(HELP.contains("read-only"));
        // The two local behaviours the help names, and the stream it follows.
        assert!(HELP.contains("quick boots fill the"));
        assert!(HELP.contains("exports a transcript"));
        assert!(HELP.contains("the session's CSV"));
        assert!(HELP.contains("GET /api/v0/events"));
        // And the hub half: what it reads, and what its absence means.
        assert!(HELP.contains("--hub-url"));
        assert!(HELP.contains("APZ_DESKTOP_HUB"));
        assert!(HELP.contains("registered, not connected"));
        // And the operator controls: the admin surface they drive, and the
        // two laws they answer to (a refusal, and nothing torn down).
        assert!(HELP.contains("--hub-admin-url"));
        assert!(HELP.contains("APZ_DESKTOP_HUB_ADMIN"));
        assert!(HELP.contains("POST /tell"));
        assert!(HELP.contains("POST /stop"));
        assert!(HELP.contains("nothing torn"));
    }

    #[test]
    fn no_hub_is_configured_by_default() {
        assert_eq!(config(&[]).hub, None);
    }

    #[test]
    fn the_hub_argument_takes_both_spellings_and_a_port() {
        assert_eq!(
            config(&["--hub-url", "127.0.0.1:19080"]).hub,
            Some(Endpoint::loopback_at("127.0.0.1", 19_080).expect("loopback"))
        );
        assert_eq!(
            config(&["--hub-url=http://localhost:19081/"]).hub,
            Some(Endpoint::loopback_at("127.0.0.1", 19_081).expect("loopback"))
        );
        assert_eq!(
            parse(args(&["--hub-url"]), &no_env).expect_err("needs a value"),
            "--hub-url needs a value"
        );
    }

    #[test]
    fn the_hub_environment_names_the_hub_and_an_argument_wins() {
        let env = |name: &str| match name {
            HUB_ENV => Some("127.0.0.1:19080".to_string()),
            _ => None,
        };
        let Invocation::Run(config) = parse(args(&[]), &env).expect("parses") else {
            panic!("not help")
        };
        assert_eq!(config.hub.expect("a hub").port(), 19_080);

        let Invocation::Run(config) =
            parse(args(&["--hub-url", "127.0.0.1:19082"]), &env).expect("parses")
        else {
            panic!("not help")
        };
        assert_eq!(config.hub.expect("a hub").port(), 19_082);

        let broken = |name: &str| match name {
            HUB_ENV => Some("not a url".to_string()),
            _ => None,
        };
        assert!(parse(args(&[]), &broken).is_err());
    }

    #[test]
    fn a_hub_url_off_the_loopback_or_without_a_port_is_refused_by_name() {
        for url in [
            "http://10.0.0.9:19080",
            "http://127.0.0.1.evil.tld:19080",
            "api.example.com:19080",
            "127.0.0.1.",
            "127.0.0.1:19080/status?a=b",
        ] {
            let error = parse(args(&["--hub-url", url]), &no_env)
                .expect_err("must be refused");
            assert!(!error.is_empty(), "{url}: {error}");
        }
        // The status surface has no default port, so a bare host is refused
        // rather than pointed at the daemon's.
        let error = parse(args(&["--hub-url", "127.0.0.1"]), &no_env)
            .expect_err("must be refused");
        assert!(error.contains("--status-port"), "{error}");
    }

    #[test]
    fn no_hub_admin_is_configured_by_default() {
        assert_eq!(config(&[]).hub_admin, None);
    }

    #[test]
    fn the_hub_admin_argument_takes_both_spellings_and_a_port() {
        assert_eq!(
            config(&["--hub-admin-url", "127.0.0.1:19081"]).hub_admin,
            Some(Endpoint::loopback_at("127.0.0.1", 19_081).expect("loopback"))
        );
        assert_eq!(
            config(&["--hub-admin-url=http://localhost:19082/"]).hub_admin,
            Some(Endpoint::loopback_at("127.0.0.1", 19_082).expect("loopback"))
        );
        assert_eq!(
            parse(args(&["--hub-admin-url"]), &no_env).expect_err("needs a value"),
            "--hub-admin-url needs a value"
        );
        // The two hub surfaces are independent: naming one leaves the other
        // off.
        let config = config(&["--hub-admin-url", "127.0.0.1:19081"]);
        assert_eq!(config.hub, None);
    }

    #[test]
    fn the_hub_admin_environment_names_the_surface_and_an_argument_wins() {
        let env = |name: &str| match name {
            HUB_ADMIN_ENV => Some("127.0.0.1:19081".to_string()),
            _ => None,
        };
        let Invocation::Run(config) = parse(args(&[]), &env).expect("parses") else {
            panic!("not help")
        };
        assert_eq!(config.hub_admin.expect("an admin").port(), 19_081);

        let Invocation::Run(config) =
            parse(args(&["--hub-admin-url", "127.0.0.1:19083"]), &env).expect("parses")
        else {
            panic!("not help")
        };
        assert_eq!(config.hub_admin.expect("an admin").port(), 19_083);

        let broken = |name: &str| match name {
            HUB_ADMIN_ENV => Some("not a url".to_string()),
            _ => None,
        };
        assert!(parse(args(&[]), &broken).is_err());
    }

    #[test]
    fn a_hub_admin_url_off_the_loopback_or_without_a_port_is_refused_by_name() {
        for url in [
            "http://10.0.0.9:19081",
            "http://127.0.0.1.evil.tld:19081",
            "api.example.com:19081",
            "127.0.0.1.",
            "127.0.0.1:19081/tell",
        ] {
            let error = parse(args(&["--hub-admin-url", url]), &no_env)
                .expect_err("must be refused");
            assert!(!error.is_empty(), "{url}: {error}");
        }
        // The admin surface is off unless `--admin-port` asked for it, so a
        // bare host names that flag rather than defaulting to any port.
        let error = parse(args(&["--hub-admin-url", "127.0.0.1"]), &no_env)
            .expect_err("must be refused");
        assert!(error.contains("--admin-port"), "{error}");
    }

    #[test]
    fn the_url_argument_takes_a_port_in_both_spellings() {
        assert_eq!(config(&["--url", "http://127.0.0.1:9000"]).endpoint.port(), 9000);
        assert_eq!(config(&["--url=http://127.0.0.1:9001"]).endpoint.port(), 9001);
        assert_eq!(config(&["--url", "localhost:9002"]).endpoint.port(), 9002);
    }

    #[test]
    fn the_environment_names_the_daemon_and_an_argument_wins() {
        let env = |name: &str| match name {
            URL_ENV => Some("http://127.0.0.1:9003".to_string()),
            TIMEOUT_ENV => Some("9".to_string()),
            _ => None,
        };
        let Invocation::Run(config) = parse(args(&[]), &env).expect("parses") else {
            panic!("not help")
        };
        assert_eq!(config.endpoint.port(), 9003);
        assert_eq!(config.timeout, Duration::from_secs(9));

        let Invocation::Run(config) =
            parse(args(&["--url", "127.0.0.1:9004", "--timeout-secs", "2"]), &env)
                .expect("parses")
        else {
            panic!("not help")
        };
        assert_eq!(config.endpoint.port(), 9004);
        assert_eq!(config.timeout, Duration::from_secs(2));
    }

    #[test]
    fn the_timeout_bounds_are_refused_with_a_reason() {
        assert_eq!(config(&["--timeout-secs", "1"]).timeout, Duration::from_secs(1));
        assert_eq!(
            config(&["--timeout-secs", "600"]).timeout,
            Duration::from_secs(600)
        );
        for value in ["0", "601", "-2", "five", ""] {
            let error = parse(
                args(&["--timeout-secs", value]),
                &no_env,
            )
            .expect_err("must be refused");
            assert!(!error.is_empty(), "{value}");
        }
        assert_eq!(
            parse(args(&["--timeout-secs"]), &no_env).expect_err("needs a value"),
            "--timeout-secs needs a value"
        );
    }

    #[test]
    fn a_non_loopback_url_is_refused_by_name() {
        let error = parse(args(&["--url", "http://10.0.0.9:8899"]), &no_env)
            .expect_err("must be refused");
        assert!(error.contains("10.0.0.9"), "{error}");
        assert!(error.contains("127.0.0.1"), "{error}");
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        let error = parse(args(&["--port", "8899"]), &no_env).expect_err("must be refused");
        assert_eq!(error, "unknown argument \"--port\"");
    }

    #[test]
    fn help_is_recognized_in_both_spellings() {
        assert_eq!(parse(args(&["-h"]), &no_env).expect("parses"), Invocation::Help);
        assert_eq!(
            parse(args(&["--help"]), &no_env).expect("parses"),
            Invocation::Help
        );
        assert_eq!(
            parse(args(&["--url", "http://127.0.0.1:1", "--help"]), &no_env).expect("parses"),
            Invocation::Help
        );
        assert!(HELP.contains("APZ_DESKTOP_URL"));
    }

    #[test]
    fn a_broken_environment_value_is_refused() {
        let env = |name: &str| match name {
            TIMEOUT_ENV => Some("later".to_string()),
            _ => None,
        };
        assert!(parse(args(&[]), &env).is_err());
    }
}
