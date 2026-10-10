//! `noded` - the AgentPlaza node daemon v0.
//!
//! ```text
//! noded [--port 8899] [--kernel /boot/vmlinuz-linux]
//!       [--initrd target/guest/initrd.x86_64.gz]
//! ```
//!
//! Binds `127.0.0.1` only and serves the run API plus the console's static
//! files; see the crate documentation for the endpoints. The guest images and
//! the console directory are read relative to the working directory, so the
//! daemon is started from the repository root.

use std::path::PathBuf;
use std::process::ExitCode;

use noded::{Config, Server};

const USAGE: &str = "\
usage: noded [--port PORT] [--kernel PATH] [--initrd PATH]

  --port PORT    port to listen on, on 127.0.0.1 (default 8899)
  --kernel PATH  guest kernel image (default /boot/vmlinuz-linux)
  --initrd PATH  guest initrd (default target/guest/initrd.x86_64.gz)
  -h, --help     print this help

Serves GET/POST /api/v0/runs, GET/DELETE /api/v0/runs/{id},
POST /api/v0/runs/{id}/input and the console's files from crates/noded/console.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("noded: {message}");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let config = parse_args(args)?;
    let server = Server::start(config.clone()).map_err(|err| format!("cannot listen: {err}"))?;
    println!(
        "noded: listening on http://127.0.0.1:{} (kernel {}, initrd {}, console {})",
        server.port(),
        config.kernel.display(),
        config.initrd.display(),
        config.console_dir.display()
    );
    server.wait().map_err(|err| format!("server error: {err}"))
}

/// Reads `--port`, `--kernel` and `--initrd`, leaving the rest at its default.
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("missing value for {flag}"));
        match argument.as_str() {
            "--port" => {
                config.port = value("--port")?
                    .parse()
                    .map_err(|err| format!("--port: {err}"))?;
            }
            "--kernel" => config.kernel = PathBuf::from(value("--kernel")?),
            "--initrd" => config.initrd = PathBuf::from(value("--initrd")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_contracts_defaults() {
        let config = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(config.port, 8899);
        assert_eq!(config.kernel, PathBuf::from("/boot/vmlinuz-linux"));
        assert_eq!(
            config.initrd,
            PathBuf::from("target/guest/initrd.x86_64.gz")
        );
    }

    #[test]
    fn flags_override_the_defaults() {
        let config = parse_args(
            ["--port", "9000", "--kernel", "/k", "--initrd", "/i"]
                .map(String::from)
                .to_vec(),
        )
        .expect("well-formed flags");
        assert_eq!(config.port, 9000);
        assert_eq!(config.kernel, PathBuf::from("/k"));
        assert_eq!(config.initrd, PathBuf::from("/i"));
    }

    #[test]
    fn a_bad_command_line_is_refused() {
        for args in [
            vec!["--port".to_string()],
            vec!["--port".to_string(), "nope".to_string()],
            vec!["--kernel".to_string()],
            vec!["--what".to_string()],
        ] {
            assert!(parse_args(args.clone()).is_err(), "{args:?}");
        }
    }
}
