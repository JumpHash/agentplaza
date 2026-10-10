//! The command line: which daemon to read, and how long a call may take.
//!
//! `--url` and `APZ_DESKTOP_URL` both name the daemon; an argument wins over
//! the environment, and both default to `http://127.0.0.1:8899`. Only a
//! loopback host is accepted, and only an exact one — see
//! [`Endpoint::from_url`](crate::api::Endpoint::from_url).

use std::time::Duration;

use crate::api::{DEFAULT_TIMEOUT, Endpoint};
use crate::model::{DEFAULT_REFRESH_MS, MAX_REFRESH_MS, MIN_REFRESH_MS};

/// The environment variable naming the daemon.
pub const URL_ENV: &str = "APZ_DESKTOP_URL";

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
    /// The bound on one call.
    pub timeout: Duration,
    /// How often a healthy daemon is polled, in milliseconds.
    pub refresh_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint: Endpoint::default(),
            timeout: DEFAULT_TIMEOUT,
            refresh_ms: DEFAULT_REFRESH_MS,
        }
    }
}

/// The usage text.
pub const HELP: &str = "\
agentplaza-desktop — the AgentPlaza fleet register

USAGE:
    agentplaza-desktop [--url <url>] [--refresh-ms <milliseconds>]
                       [--timeout-secs <seconds>]

OPTIONS:
    --url <url>            the node daemon to read; http://127.0.0.1:8899 by
                           default. Only a loopback host is accepted.
    --refresh-ms <n>       how often a healthy daemon is polled, 250–30000;
                           2000 by default. A failing daemon backs off from
                           here, doubling to a 30 s ceiling.
    --timeout-secs <n>     how long one call may take, 1–600; 5 by default
    -h, --help             print this help

ENVIRONMENT:
    APZ_DESKTOP_URL           the same as --url
    APZ_DESKTOP_REFRESH_MS    the same as --refresh-ms
    APZ_DESKTOP_TIMEOUT_SECS  the same as --timeout-secs

KEYS:
    r                      the fleet register
    m                      the machine (GET /api/v0/machine)
    f                      the fleet map
    t                      the session's telemetry, one observation per poll
    ?                      the help sheet (Escape or ? closes it)
    Escape                 back to the register
    up / down              walk the console's command history

The dashboard polls GET /api/v0/runs and GET /api/v0/machine, follows
GET /api/v0/runs/{id} for the selected run, writes to a VM's console with
POST /api/v0/runs/{id}/input, spawns runs with POST /api/v0/runs and kills
them with DELETE /api/v0/runs/{id}. The spawn form's quick boots fill the
form; the run pane exports a transcript, and the telemetry pane the session's
CSV, to the path in its box with plain std::fs.
";

/// Parses the arguments, with `env` standing in for the process environment.
pub fn parse<I, F>(args: I, env: &F) -> Result<Invocation, String>
where
    I: IntoIterator<Item = String>,
    F: Fn(&str) -> Option<String>,
{
    let mut url = env(URL_ENV);
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
    let timeout = match timeout_secs {
        Some(secs) => Duration::from_secs(secs),
        None => DEFAULT_TIMEOUT,
    };
    Ok(Invocation::Run(Config {
        endpoint,
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
        // The two local behaviours the help names.
        assert!(HELP.contains("quick boots fill the"));
        assert!(HELP.contains("exports a transcript"));
        assert!(HELP.contains("the session's\nCSV"));
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
