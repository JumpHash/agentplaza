//! `projects env`: a project's environments, where it runs (`crates/apid/src/environments.rs`). An environment has a kind,
//! an address, a track (what it runs: each change that lands on main, each signed release, or what is deployed by
//! hand), variables, secrets kept in the vault (their values are never read back) and an optional deploy hook.

use std::io::{IsTerminal, Write};

use clap::Subcommand;
use serde_json::{Map, Value, json};

use super::{array, dash};
use crate::ctx::{self, Ctx, confirm};

/// The kinds an environment takes (the hub's).
pub const KINDS: [&str; 5] = ["production", "staging", "preview", "development", "other"];
/// The tracks an environment takes (the hub's).
const TRACKS: [&str; 3] = ["main", "releases", "manual"];
/// The most characters a variable's value takes (the hub's).
const VALUE_MAX: usize = 4000;
/// What an environment's name must look like, for the message when it does not.
const NAME_RULE: &str =
    "an environment's name is lower-case letters, digits and dashes, starting with a letter";

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Make an environment, or change one. The kind is always given; each other option changes its field only when it
    /// is given, so everything else stays as it is.
    Set {
        /// The environment's name: lower-case letters, digits and dashes, starting with a letter.
        name: String,
        /// production, staging, preview, development or other.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(KINDS))]
        kind: String,
        /// Its address (`--url ""` takes it away).
        #[arg(long)]
        url: Option<String>,
        /// What it runs: main (each change that lands on it), releases (each signed release) or manual (what is
        /// deployed by hand). Production runs releases, staging and preview run main, and the rest run by hand, unless
        /// this says otherwise.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(TRACKS))]
        track: Option<String>,
        /// Protected: a deploy waits for the owner's approval.
        #[arg(long, conflicts_with = "open")]
        protected: bool,
        /// Not protected: its deploys need no approval.
        #[arg(long)]
        open: bool,
        /// A variable as NAME=VALUE; repeat for more. The variables already set stay, and a name given again takes the
        /// new value.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// The URL of the deploy hook, which the vault keeps.
        #[arg(long, value_name = "URL", conflicts_with = "no_hook")]
        hook: Option<String>,
        /// Remove the deploy hook.
        #[arg(long)]
        no_hook: bool,
        /// Notes for the team (`--notes ""` clears them).
        #[arg(long)]
        notes: Option<String>,
    },
    /// Remove an environment. Asks to confirm unless --yes.
    Rm {
        /// The environment's name.
        name: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Deploy to an environment: the version given, else the newest signed release (on a releases track) or the head of
    /// main.
    Deploy {
        /// The environment's name.
        name: String,
        /// The version to deploy (clap's `--version` is taken, so this is a plain word).
        #[arg(value_name = "VERSION")]
        release: Option<String>,
    },
    /// Put a secret in an environment's vault. Its value is read from the terminal with the echo off, or from stdin
    /// when stdin is piped. `--remove` takes the secret out instead.
    Secret {
        /// The environment's name.
        name: String,
        /// The secret's name, such as STRIPE_KEY: letters, digits and _, starting with a letter.
        key: String,
        /// Take the secret out of the vault instead.
        #[arg(long)]
        remove: bool,
    },
}

/// What `env set` changes. The kind is always given; every other field is changed only when it is given.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: String,
    /// `""` takes the address away.
    pub url: Option<String>,
    pub track: Option<String>,
    pub protected: Option<bool>,
    /// `NAME=VALUE` each; they are added to the variables already set.
    pub vars: Vec<String>,
    /// `Some(Some(url))` sets the deploy hook, `Some(None)` removes it, and `None` keeps it.
    pub hook: Option<Option<String>>,
    pub notes: Option<String>,
}

/// Whether `name` is an environment's name: lower-case letters, digits and dashes, starting with a letter (at most 40).
pub fn valid_env_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Whether `name` is a secret's or a variable's name: letters, digits and `_`, starting with a letter (at most 64).
pub fn valid_secret_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// A `NAME=VALUE` variable. The value may be empty; it is at most 4000 characters.
pub fn parse_var(pair: &str) -> Result<(String, String), String> {
    let (name, value) = pair
        .split_once('=')
        .ok_or_else(|| format!("{pair:?} is not NAME=VALUE"))?;
    if !valid_secret_name(name) {
        return Err(format!(
            "{name:?} is not a variable name: letters, digits and _, starting with a letter"
        ));
    }
    if value.chars().count() > VALUE_MAX {
        return Err(format!(
            "the value of {name} is at most {VALUE_MAX} characters"
        ));
    }
    Ok((name.to_string(), value.to_string()))
}

/// An environment for `projects adopt --env NAME=KIND[,URL]`: its name and kind, and its address when one is given.
pub fn parse_env_spec(spec: &str) -> Result<Value, String> {
    let (name, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("{spec:?} is not NAME=KIND[,URL]"))?;
    let (kind, url) = match rest.split_once(',') {
        Some((kind, url)) => (kind, Some(url.trim())),
        None => (rest, None),
    };
    if !valid_env_name(name) {
        return Err(format!("{name:?}: {NAME_RULE}"));
    }
    if !KINDS.contains(&kind) {
        return Err(format!("{kind:?} is not a kind: use {}", KINDS.join(", ")));
    }
    let mut env = json!({ "name": name, "kind": kind });
    if let Some(url) = url.filter(|url| !url.is_empty()) {
        env["url"] = json!(url);
    }
    Ok(env)
}

/// The `PUT` body that makes or changes an environment. The hub clears any field the body leaves out, so the fields
/// not given keep what `existing` (the environment as it is now) holds: the address, track, variables and notes.
/// `protected` and the hook are left out when not given, which keeps them too.
pub fn set_body(change: &Change, existing: Option<&Value>) -> Result<Value, String> {
    if !KINDS.contains(&change.kind.as_str()) {
        return Err(format!(
            "kind is one of {}, not {:?}",
            KINDS.join(", "),
            change.kind
        ));
    }
    let kept = |key: &str| {
        existing
            .and_then(|env| env.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut body = json!({ "kind": change.kind });
    if let Some(url) = change.url.clone().or_else(|| kept("url")) {
        body["url"] = json!(url);
    }
    if let Some(track) = change.track.clone().or_else(|| kept("track")) {
        body["track"] = json!(track);
    }
    if let Some(protected) = change.protected {
        body["protected"] = json!(protected);
    }
    let mut variables: Map<String, Value> = existing
        .and_then(|env| env.get("variables"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for pair in &change.vars {
        let (name, value) = parse_var(pair)?;
        variables.insert(name, json!(value));
    }
    body["variables"] = Value::Object(variables);
    body["notes"] = json!(
        change
            .notes
            .clone()
            .or_else(|| kept("notes"))
            .unwrap_or_default()
    );
    match &change.hook {
        Some(Some(url)) => body["hook"] = json!(url),
        Some(None) => body["hook"] = Value::Null,
        None => {}
    }
    Ok(body)
}

/// The environments as rows: name, kind, track, protected, version and address.
fn list_rows(envs: &Value) -> Vec<Vec<String>> {
    array(envs)
        .iter()
        .map(|e| {
            vec![
                ctx::s(e, "name").to_string(),
                ctx::s(e, "kind").to_string(),
                ctx::s(e, "track").to_string(),
                if e["protected"].as_bool() == Some(true) {
                    "yes"
                } else {
                    "no"
                }
                .to_string(),
                dash(ctx::text(e, "version")),
                dash(ctx::text(e, "url")),
            ]
        })
        .collect()
}

/// `projects env ID`: the project's environments; `set`, `rm`, `deploy` and `secret` change them.
pub fn run(ctx: &Ctx, id: &str, action: Option<Action>) -> Result<(), String> {
    let base = format!("/v1/projects/{}/environments", ctx::seg(id));
    match action {
        None => {
            let envs = ctx.client.get(&base)?;
            ctx.out(&envs, |v| {
                ctx::table(
                    &["name", "kind", "track", "protected", "version", "url"],
                    &list_rows(v),
                );
            });
        }
        Some(Action::Set {
            name,
            kind,
            url,
            track,
            protected,
            open,
            vars,
            hook,
            no_hook,
            notes,
        }) => {
            if !valid_env_name(&name) {
                return Err(NAME_RULE.into());
            }
            let change = Change {
                kind,
                url,
                track,
                protected: match (protected, open) {
                    (true, _) => Some(true),
                    (false, true) => Some(false),
                    (false, false) => None,
                },
                vars,
                hook: match (hook, no_hook) {
                    (Some(url), _) => Some(Some(url)),
                    (None, true) => Some(None),
                    (None, false) => None,
                },
                notes,
            };
            let envs = ctx.client.get(&base)?;
            let existing = array(&envs).iter().find(|e| ctx::s(e, "name") == name);
            let body = set_body(&change, existing)?;
            let saved = ctx
                .client
                .put(&format!("{base}/{}", ctx::seg(&name)), &body)?;
            ctx.out(&saved, |e| {
                println!(
                    "saved environment {name} ({}, runs {})",
                    ctx::s(e, "kind"),
                    ctx::s(e, "track")
                )
            });
        }
        Some(Action::Rm { name, yes }) => {
            if !confirm(
                yes,
                &format!("Remove environment {name} from project {id}?"),
            )? {
                println!("cancelled");
                return Ok(());
            }
            let gone = ctx.client.delete(&format!("{base}/{}", ctx::seg(&name)))?;
            ctx.out(&gone, |_| {
                println!("removed environment {name} from project {id}")
            });
        }
        Some(Action::Deploy { name, release }) => {
            let body = match release.filter(|v| !v.trim().is_empty()) {
                Some(version) => json!({ "version": version }),
                None => json!({}),
            };
            let deploy = ctx
                .client
                .post(&format!("{base}/{}/deploy", ctx::seg(&name)), &body)?;
            ctx.out(&deploy, |d| {
                println!(
                    "deploy {} of {} to {name}: {}",
                    ctx::s(d, "id"),
                    ctx::text(d, "version"),
                    ctx::s(d, "status")
                );
                let detail = ctx::s(d, "detail");
                if !detail.is_empty() {
                    println!("{detail}");
                }
            });
        }
        Some(Action::Secret { name, key, remove }) => {
            if !valid_secret_name(&key) {
                return Err(format!(
                    "{key:?} is not a secret's name: letters, digits and _, starting with a letter"
                ));
            }
            let path = format!("{base}/{}/secrets/{}", ctx::seg(&name), ctx::seg(&key));
            if remove {
                let gone = ctx.client.delete(&path)?;
                ctx.out(&gone, |_| {
                    println!("removed secret {key} from environment {name}")
                });
            } else {
                let value = secret_value(&format!("value of {key} for {name}: "))?;
                let saved = ctx.client.put(&path, &json!({ "value": value }))?;
                ctx.out(&saved, |_| {
                    println!("saved secret {key} for environment {name}")
                });
            }
        }
    }
    Ok(())
}

/// A secret from the terminal, with the echo off (the prompt is shown there); from stdin, one line and no prompt, when
/// stdin is not a terminal. It is never an argument and never printed back.
fn secret_value(prompt: &str) -> Result<String, String> {
    let line = if std::io::stdin().is_terminal() {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        hidden_line()?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
        buf
    };
    let value = line.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        return Err("nothing was entered".into());
    }
    Ok(value)
}

/// One line from the terminal with the echo turned off; the settings are put back afterwards.
#[cfg(unix)]
fn hidden_line() -> Result<String, String> {
    let fd = libc::STDIN_FILENO;
    // SAFETY: termios is plain old data, and tcgetattr fills it in before it is read.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd is the terminal on stdin and `saved` is a valid out-parameter.
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return Err("cannot read the terminal settings".into());
    }
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    // SAFETY: as above; `quiet` is a copy of the settings just read, with echo off.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
        return Err("cannot turn off the echo".into());
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    // SAFETY: restores the settings read above.
    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, &saved);
    }
    eprintln!();
    read.map_err(|e| format!("reading the prompt: {e}"))?;
    Ok(line)
}

#[cfg(not(unix))]
fn hidden_line() -> Result<String, String> {
    Err("a hidden prompt needs a Unix terminal here: pipe the value on stdin".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(kind: &str) -> Change {
        Change {
            kind: kind.into(),
            ..Change::default()
        }
    }

    #[test]
    fn a_new_environment_sends_only_what_is_given() {
        assert_eq!(
            set_body(&change("production"), None).unwrap(),
            json!({ "kind": "production", "variables": {}, "notes": "" })
        );
    }

    #[test]
    fn a_change_keeps_what_it_does_not_name() {
        let existing = json!({
            "name": "production", "kind": "production", "url": "https://shop.example",
            "track": "releases", "protected": true, "variables": { "REGION": "eu" }, "notes": "live"
        });
        let mut edit = change("production");
        edit.vars = vec!["SITE=1".into()];
        assert_eq!(
            set_body(&edit, Some(&existing)).unwrap(),
            json!({
                "kind": "production", "url": "https://shop.example", "track": "releases",
                "variables": { "REGION": "eu", "SITE": "1" }, "notes": "live"
            }),
            "protected and the hook are left out, so the hub keeps them"
        );
    }

    #[test]
    fn an_address_is_taken_away_with_an_empty_one_and_the_hook_with_null() {
        let existing = json!({ "name": "staging", "kind": "staging", "url": "https://old.example", "track": "main" });
        let mut edit = change("staging");
        edit.url = Some(String::new());
        edit.hook = Some(None);
        edit.track = Some("manual".into());
        let body = set_body(&edit, Some(&existing)).unwrap();
        assert_eq!(body["url"], "");
        assert_eq!(body["hook"], Value::Null);
        assert_eq!(body["track"], "manual");
    }

    #[test]
    fn a_bad_variable_is_refused_before_anything_is_sent() {
        let mut edit = change("other");
        edit.vars = vec!["1X=2".into()];
        assert!(set_body(&edit, None).is_err());
        assert!(set_body(&change("cloud"), None).is_err());
    }

    #[test]
    fn an_adopted_environment_is_a_name_a_kind_and_an_address() {
        assert_eq!(
            parse_env_spec("production=production,https://shop.example").unwrap(),
            json!({ "name": "production", "kind": "production", "url": "https://shop.example" })
        );
        assert_eq!(
            parse_env_spec("staging=staging").unwrap(),
            json!({ "name": "staging", "kind": "staging" })
        );
        assert!(
            parse_env_spec("Prod=production").is_err(),
            "a name is lower case"
        );
        assert!(
            parse_env_spec("prod=live").is_err(),
            "a kind is one of the five"
        );
        assert!(parse_env_spec("prod").is_err(), "NAME=KIND is required");
    }

    #[test]
    fn names_follow_the_hub() {
        assert!(valid_env_name("eu-west-2"));
        assert!(!valid_env_name("2eu"));
        assert!(!valid_env_name("eu_west"));
        assert!(valid_secret_name("STRIPE_KEY"));
        assert!(!valid_secret_name("STRIPE-KEY"));
        assert_eq!(
            parse_var("A=").unwrap(),
            ("A".to_string(), String::new()),
            "an empty value is a value"
        );
    }

    #[test]
    fn the_environments_list_shows_the_track_and_the_address_or_dashes() {
        let envs = json!([
            { "name": "production", "kind": "production", "track": "releases", "protected": true,
              "version": "1.4.0", "url": "https://shop.example" },
            { "name": "preview", "kind": "preview", "track": "main", "protected": false, "version": null, "url": null }
        ]);
        let rows = list_rows(&envs);
        assert_eq!(rows[0][3..], ["yes", "1.4.0", "https://shop.example"]);
        assert_eq!(rows[1][3..], ["no", "-", "-"]);
    }
}
