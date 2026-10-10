//! The command line: which daemon to read, and how long a call may take.
//!
//! `--url` and `APZ_DESKTOP_URL` both name the daemon; an argument wins over
//! the environment, and both default to `http://127.0.0.1:8899`. Only a
//! loopback host is accepted, and only an exact one — see
//! [`Endpoint::from_url`](crate::api::Endpoint::from_url).

use std::time::Duration;

use crate::api::{DEFAULT_TIMEOUT, Endpoint};

/// The environment variable naming the daemon.
pub const URL_ENV: &str = "APZ_DESKTOP_URL";

/// The environment variable bounding one call, in seconds.
pub const TIMEOUT_ENV: &str = "APZ_DESKTOP_TIMEOUT_SECS";

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

/// Where the dashboard reads from, and how long it waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The daemon.
    pub endpoint: Endpoint,
    /// The bound on one call.
    pub timeout: Duration,
}

/// The usage text.
pub const HELP: &str = "\
agentplaza-desktop — the AgentPlaza fleet register

USAGE:
    agentplaza-desktop [--url <url>] [--timeout-secs <seconds>]

OPTIONS:
    --url <url>            the node daemon to read; http://127.0.0.1:8899 by
                           default. Only a loopback host is accepted.
    --timeout-secs <n>     how long one call may take, 1–600; 5 by default
    -h, --help             print this help

ENVIRONMENT:
    APZ_DESKTOP_URL           the same as --url
    APZ_DESKTOP_TIMEOUT_SECS  the same as --timeout-secs

The dashboard polls GET /api/v0/runs every two seconds (backing off to 30 s
while the daemon refuses or is unreachable), follows GET /api/v0/runs/{id} for
the selected run, spawns runs with POST /api/v0/runs and kills them with
DELETE /api/v0/runs/{id}.
";

/// Parses the arguments, with `env` standing in for the process environment.
pub fn parse<I, F>(args: I, env: &F) -> Result<Invocation, String>
where
    I: IntoIterator<Item = String>,
    F: Fn(&str) -> Option<String>,
{
    let mut url = env(URL_ENV);
    let mut timeout_secs = env(TIMEOUT_ENV).map(|value| parse_secs(&value)).transpose()?;

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
    Ok(Invocation::Run(Config { endpoint, timeout }))
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
