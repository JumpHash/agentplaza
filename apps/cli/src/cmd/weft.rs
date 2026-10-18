//! `agentplaza weft`: Weft, the plaza's version control (history, lanes, releases, git export). Every word after
//! `weft` goes to the `weft` command line untouched (`agentplaza weft log 'landed()' --repo x`). That command talks
//! to the same hub as this one: its Weft (`WEFT_HUB`), the owner's token and pin, and the owner's key when this
//! machine has one (`WEFT_KEY`; without it the hub signs for the owner from the console). `agentplaza weft help`
//! lists Weft's commands.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ctx::Ctx;

/// `agentplaza weft`: the `weft` command and its arguments, passed through as they are.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// The `weft` command and its arguments (`agentplaza weft help` lists them).
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        num_args = 0..,
        value_name = "ARGS"
    )]
    pub args: Vec<String>,
}

/// Runs the `weft` command line with these arguments, on this command's connection to the hub.
pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let client = &ctx.client;
    let hub = format!("{}/v1/weft", client.base());
    let bearer = client
        .target
        .bearer
        .as_deref()
        .filter(|token| !token.is_empty());
    let key = owner_key_file();
    let key = key.is_file().then_some(key);
    let env = env_for(&hub, bearer, client.target.pin, key.as_deref());
    let cwd = std::env::current_dir().map_err(|e| format!("the working directory: {e}"))?;
    weft::cli::run_in(weft_args(&args.args, ctx.json), cwd, env)
}

/// The owner's device key on this machine, beside `cli.json` in the config directory.
fn owner_key_file() -> PathBuf {
    apz_os::dirs::config().join("owner.key")
}

/// The variables the `weft` command line reads from its environment. They win over the process environment there, so
/// they are this command's own connection; a value this command does not have is left to the process environment.
fn env_for(
    hub: &str,
    bearer: Option<&str>,
    pin: Option<[u8; 32]>,
    key: Option<&Path>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("WEFT_HUB".to_string(), hub.to_string());
    if let Some(bearer) = bearer {
        env.insert("WEFT_BEARER".to_string(), bearer.to_string());
    }
    if let Some(pin) = pin {
        let hex: String = pin.iter().map(|byte| format!("{byte:02x}")).collect();
        env.insert("WEFT_PIN".to_string(), hex);
    }
    if let Some(key) = key {
        env.insert("WEFT_KEY".to_string(), key.display().to_string());
    }
    env
}

/// The words for the `weft` command line. The global `--json` is taken by this command before Weft sees it, so it is
/// given back to Weft here.
fn weft_args(args: &[String], json: bool) -> Vec<String> {
    let mut out = args.to_vec();
    if json && !out.iter().any(|arg| arg == "--json") {
        out.push("--json".to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn the_connection_reaches_weft_as_its_own_variables() {
        let env = env_for(
            "http://127.0.0.1:7700/v1/weft",
            Some("token"),
            Some([0xab; 32]),
            Some(Path::new("/keys/owner.key")),
        );
        assert_eq!(env["WEFT_HUB"], "http://127.0.0.1:7700/v1/weft");
        assert_eq!(env["WEFT_BEARER"], "token");
        assert_eq!(env["WEFT_PIN"], "ab".repeat(32));
        assert_eq!(env["WEFT_KEY"], "/keys/owner.key");

        // With nothing else known, only the hub is set: the process environment still applies to the rest.
        let bare = env_for("http://hub.test/v1/weft", None, None, None);
        assert_eq!(
            bare.keys().map(String::as_str).collect::<Vec<_>>(),
            ["WEFT_HUB"]
        );
    }

    #[test]
    fn the_words_reach_weft_unchanged() {
        let args = words(&["log", "landed()", "--repo", "x", "--limit", "5"]);
        assert_eq!(weft_args(&args, false), args);
        // A `--json` given before `weft` is handed on, once.
        let asked = words(&["status", "--json"]);
        assert_eq!(weft_args(&asked, true), asked);
        assert_eq!(
            weft_args(&words(&["status"]), true),
            words(&["status", "--json"])
        );
    }

    #[test]
    fn clap_keeps_every_word_after_the_command_for_weft() {
        let cli = crate::Cli::try_parse_from([
            "agentplaza",
            "weft",
            "log",
            "landed()",
            "--repo",
            "x",
            "--json",
        ])
        .unwrap();
        match cli.command {
            crate::Command::Weft(args) => assert_eq!(
                args.args,
                words(&["log", "landed()", "--repo", "x", "--json"])
            ),
            other => panic!("not the weft command: {other:?}"),
        }
    }
}
