//! `agentplaza vault`: the hub's encrypted vault. A value never comes from the command line: `set`,
//! `unseal` and `rekey` read it from stdin when stdin is not a terminal, else from a prompt with no
//! echo. `import` moves a `.env` file's keys into the vault; `--shred` then removes the file, once
//! the hub has every key.

mod files;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::ctx::{self, confirm, Ctx};

const IMPORT_WARNING: &str = "warning: .env files are discouraged: the keys in one sit as plain text on disk.\n\
     Keys belong in the hub's encrypted vault. Add one with `agentplaza vault set NAME`.";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The vault: its state, its slots, its entry count and its nodes. Never values.
    Status,
    /// Unseal the vault. Asks for the passphrase (or the recovery code with --recovery). --keyfile
    /// unseals from the key file and asks for nothing.
    Unseal {
        /// Unseal with the recovery code instead of the passphrase.
        #[arg(long, conflicts_with = "keyfile")]
        recovery: bool,
        /// Unseal from the key file (no secret is asked for).
        #[arg(long, conflicts_with = "recovery")]
        keyfile: bool,
    },
    /// Seal the vault: nothing can be read until the next unseal.
    Seal,
    /// The entries: names, kinds, scopes, bindings and versions. Never values.
    List,
    /// The audit log. --verify recomputes every link of its chain on the hub.
    Audit {
        /// Recompute the whole chain (otherwise the hub's last known answer).
        #[arg(long)]
        verify: bool,
        /// Only lines after this sequence number.
        #[arg(long, default_value_t = 0)]
        after: u64,
        /// How many lines to show (1 to 500).
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=500))]
        limit: u64,
    },
    /// A new root key: every slot is re-wrapped. Asks for the passphrase or recovery code of each
    /// other passphrase or recovery slot; press Enter to skip one that is not needed.
    Rekey,
    /// A new recovery code, shown once. It replaces the old one.
    Recovery,
    /// Store a secret. Its value is read from stdin (when it is not a terminal) or a hidden prompt.
    Set {
        /// The name, e.g. ANTHROPIC_API_KEY: upper case letters, digits and underscores.
        name: String,
        /// What it is: api_key (the default), token, password or generic.
        #[arg(long, value_enum, default_value_t = Kind::ApiKey)]
        kind: Kind,
        /// A short note shown with it.
        #[arg(long, default_value = "")]
        label: String,
        /// Every node of the plaza (the default). `plaza` is the only word.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["plaza"]), conflicts_with = "nodes")]
        scope: Option<String>,
        /// Only these nodes, e.g. n0,n3.
        #[arg(long, value_delimiter = ',', conflicts_with = "scope")]
        nodes: Option<Vec<String>>,
        /// A binding it serves: provider:anthropic, connector:slack, mcp:github, tool:web-search or
        /// hf. Repeatable, or comma-separated.
        #[arg(long = "bind", value_delimiter = ',')]
        bind: Vec<String>,
    },
    /// Delete an entry by its id (see `list`). A .env import will not bring its name back.
    Rm {
        /// The entry's id, e.g. sec_...
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Move a .env file's keys into the vault. Discouraged: add a key with `set` instead.
    Import {
        /// The .env file.
        file: PathBuf,
        /// Only these nodes, e.g. n0,n3 (default: every node of the plaza).
        #[arg(long, value_delimiter = ',')]
        nodes: Option<Vec<String>>,
        /// NAME=binding,binding for one key of the file (repeatable).
        #[arg(long = "bind", value_name = "NAME=BINDINGS")]
        bind: Vec<String>,
        /// Once every key is in the vault, overwrite the file with zeros and delete it.
        #[arg(long)]
        shred: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum Kind {
    ApiKey,
    Token,
    Password,
    Generic,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::ApiKey => "api_key",
            Kind::Token => "token",
            Kind::Password => "password",
            Kind::Generic => "generic",
        }
    }
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Status => {
            let v = ctx.client.get("/v1/vault")?;
            ctx.out(&v, print_status);
        }
        Cmd::Unseal { recovery, keyfile } => {
            let body = if keyfile {
                json!({})
            } else if recovery {
                json!({ "recovery_code": secret("recovery code: ")? })
            } else {
                json!({ "passphrase": secret("passphrase: ")? })
            };
            let v = ctx.client.post("/v1/vault/unseal", &body)?;
            ctx.out(&v, print_status);
        }
        Cmd::Seal => {
            let v = ctx.client.post("/v1/vault/seal", &json!({}))?;
            ctx.out(&v, print_status);
        }
        Cmd::List => {
            let v = ctx.client.get("/v1/vault/secrets")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v).iter().map(entry_row).collect();
                ctx::table(&["id", "name", "kind", "scope", "bindings", "version", "uses", "last used"], &rows);
            });
        }
        Cmd::Audit { verify, after, limit } => {
            let mut path = format!("/v1/vault/audit?after={after}&limit={limit}");
            if verify {
                path.push_str("&verify=1");
            }
            let v = ctx.client.get(&path)?;
            let chain_ok = v.get("chain_ok").cloned().unwrap_or(Value::Null);
            ctx.out(&v, print_audit);
            if chain_ok == Value::Bool(false) {
                return Err("the audit chain is broken: a line was changed or removed after it was written".into());
            }
        }
        Cmd::Rekey => {
            let status = ctx.client.get("/v1/vault")?;
            let mut secrets = serde_json::Map::new();
            for slot in array(&status["slots"]) {
                let kind = ctx::s(slot, "kind");
                if kind != "passphrase" && kind != "recovery" {
                    continue;
                }
                let id = ctx::s(slot, "id");
                let what = if kind == "recovery" { "recovery code" } else { "passphrase" };
                let value = read_line(&format!("{what} of {id} (Enter if it is not needed): "))?;
                if !value.is_empty() {
                    secrets.insert(id.to_string(), Value::String(value));
                }
            }
            let v = ctx.client.post("/v1/vault/rekey", &json!({ "secrets": secrets }))?;
            ctx.out(&v, print_status);
        }
        Cmd::Recovery => {
            let v = ctx.client.post("/v1/vault/recovery", &json!({}))?;
            ctx.out(&v, |v| {
                eprintln!("the recovery code is shown once: keep it somewhere safe");
                println!("{}", ctx::s(v, "recovery_code"));
            });
        }
        Cmd::Set { name, kind, label, scope, nodes, bind } => {
            if !valid_name(&name) {
                return Err(format!("{name} is not a name: 2 to 64 upper case letters, digits or _, starting with a letter"));
            }
            if scope.is_some() && nodes.is_some() {
                return Err("--scope and --nodes are one choice: give one".into());
            }
            let scope = scope_value(nodes.as_deref())?;
            let bindings = parse_bindings(&bind)?;
            let value = secret(&format!("value for {name}: "))?;
            let body = json!({
                "name": name,
                "kind": kind.word(),
                "label": label,
                "value": value,
                "scope": scope,
                "bindings": bindings,
            });
            let v = ctx.client.post("/v1/vault/secrets", &body)?;
            ctx.out(&v, |v| println!("stored {} as {} ({})", ctx::s(v, "name"), ctx::s(v, "id"), scope_text(&v["scope"])));
        }
        Cmd::Rm { id, yes } => {
            if !confirm(yes, &format!("Delete entry {id}? Its value is gone, and a .env import will not bring it back."))? {
                println!("cancelled");
                return Ok(());
            }
            let v = ctx.client.delete(&format!("/v1/vault/secrets/{}", ctx::seg(&id)))?;
            ctx.out(&v, |_| println!("removed {id}"));
        }
        Cmd::Import { file, nodes, bind, shred } => import(ctx, &file, nodes.as_deref(), &bind, shred)?,
    }
    Ok(())
}

/// `agentplaza vault import`: the warning, then the file's keys to the hub; `--shred` removes the
/// file only after every key is in the vault.
fn import(ctx: &Ctx, file: &Path, nodes: Option<&[String]>, bind: &[String], shred: bool) -> Result<(), String> {
    eprintln!("{IMPORT_WARNING}");
    let scope = scope_value(nodes)?;
    let bindings = parse_bind_map(bind)?;
    let left_alone = |e: String| if shred { format!("{e}; {} was left alone", file.display()) } else { e };
    if shred {
        files::check_shreddable(file).map_err(left_alone)?;
    }
    let text = std::fs::read_to_string(file).map_err(|e| left_alone(format!("{}: {e}", file.display())))?;
    let keys: BTreeSet<String> = files::dotenv_names(&text).map_err(|e| left_alone(format!("{}: {e}", file.display())))?.into_iter().collect();
    let body = json!({ "format": "dotenv", "text": text, "scope": scope, "bindings": bindings });
    let v = ctx.client.post("/v1/vault/import", &body).map_err(left_alone)?;
    ctx.out(&v, print_import);
    if !shred {
        return Ok(());
    }
    let imported = imported_names(&v);
    let skipped = array(&v["skipped"]).len();
    if let Err(why) = shred_verdict(&keys, &imported, skipped) {
        return Err(format!("not shredding {}: {why}; the file was left alone", file.display()));
    }
    files::shred(file).map_err(|e| format!("imported, but shredding failed: {e}"))?;
    println!("shredded {}", file.display());
    Ok(())
}

/// Whether `--shred` may remove the file: the hub skipped nothing, and it confirmed every distinct
/// key of the file. Otherwise the reason.
fn shred_verdict(keys: &BTreeSet<String>, imported: &BTreeSet<String>, skipped: usize) -> Result<(), String> {
    if skipped > 0 {
        return Err(format!("{skipped} key(s) were skipped"));
    }
    match keys.iter().find(|key| !imported.contains(*key)) {
        Some(key) => Err(format!("the hub did not confirm {key}")),
        None => Ok(()),
    }
}

fn imported_names(v: &Value) -> BTreeSet<String> {
    array(&v["imported"]).iter().map(|e| ctx::s(e, "name").to_string()).collect()
}

fn print_import(v: &Value) {
    let names: Vec<&str> = array(&v["imported"]).iter().map(|e| ctx::s(e, "name")).collect();
    if names.is_empty() {
        println!("imported 0");
    } else {
        println!("imported {}: {}", names.len(), names.join(", "));
    }
    let skipped = array(&v["skipped"]);
    if !skipped.is_empty() {
        println!("skipped {}:", skipped.len());
        for s in skipped {
            println!("  {}: {}", ctx::s(s, "name"), ctx::s(s, "reason"));
        }
    }
}

fn print_status(v: &Value) {
    let slots: Vec<String> = array(&v["slots"]).iter().map(|s| format!("{} ({})", ctx::s(s, "id"), ctx::s(s, "kind"))).collect();
    let nodes: Vec<String> = array(&v["nodes"]).iter().map(|n| format!("{} ({})", ctx::s(n, "name"), ctx::s(n, "id"))).collect();
    let audit = match v.get("audit_ok") {
        Some(Value::Bool(true)) => "ok",
        Some(Value::Bool(false)) => "BROKEN",
        _ => "unknown",
    };
    let mut pairs = vec![
        ("state", ctx::s(v, "state").to_string()),
        ("entries", ctx::text(v, "entries")),
        ("slots", if slots.is_empty() { "none".to_string() } else { slots.join(", ") }),
        ("nodes", nodes.join(", ")),
        ("audit", audit.to_string()),
        ("rekeyed", ctx::when(&v["rekeyed_ms"])),
    ];
    if !v["tampered"].is_null() {
        pairs.push(("tampered", ctx::text(v, "tampered")));
    }
    ctx::fields(&pairs);
}

fn print_audit(v: &Value) {
    let rows: Vec<Vec<String>> = array(&v["entries"])
        .iter()
        .map(|e| {
            vec![
                ctx::text(e, "seq"),
                ctx::when(&e["ts_ms"]),
                ctx::s(e, "event").to_string(),
                ctx::s(e, "actor").to_string(),
                ctx::s(e, "name").to_string(),
                ctx::s(e, "binding").to_string(),
                ctx::s(e, "node").to_string(),
                ctx::s(e, "detail").to_string(),
            ]
        })
        .collect();
    ctx::table(&["seq", "at", "event", "actor", "name", "binding", "node", "detail"], &rows);
    match v.get("chain_ok") {
        Some(Value::Bool(true)) => println!("chain: ok"),
        Some(Value::Bool(false)) => println!("chain: BROKEN"),
        _ => println!("chain: unknown"),
    }
}

fn entry_row(e: &Value) -> Vec<String> {
    let current = ctx::text(e, "current");
    let fingerprint = array(&e["versions"])
        .iter()
        .find(|ver| ctx::text(ver, "v") == current)
        .map(|ver| ctx::s(ver, "fp").to_string())
        .unwrap_or_default();
    let bindings: Vec<&str> = array(&e["bindings"]).iter().filter_map(Value::as_str).collect();
    let last_used = if e["last_used_ms"].as_u64().unwrap_or(0) == 0 { "never".to_string() } else { ctx::ago(&e["last_used_ms"]) };
    vec![
        ctx::s(e, "id").to_string(),
        ctx::s(e, "name").to_string(),
        ctx::s(e, "kind").to_string(),
        scope_text(&e["scope"]),
        if bindings.is_empty() { "—".to_string() } else { bindings.join(",") },
        format!("v{current} {fingerprint}"),
        ctx::text(e, "use_count"),
        last_used,
    ]
}

/// A scope as the people read it: `plaza`, or `nodes n0,n3`.
fn scope_text(scope: &Value) -> String {
    if ctx::s(scope, "kind") == "nodes" {
        let nodes: Vec<&str> = array(&scope["nodes"]).iter().filter_map(Value::as_str).collect();
        format!("nodes {}", nodes.join(","))
    } else {
        "plaza".to_string()
    }
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A secret read with `read_line`; an empty one is refused.
fn secret(prompt: &str) -> Result<String, String> {
    let value = read_line(prompt)?;
    if value.is_empty() {
        return Err("nothing was entered".into());
    }
    Ok(value)
}

/// One line of input without its newline: from stdin when stdin is not a terminal (no prompt is
/// shown), else from a prompt on the terminal with the echo off. Empty at the end of the input.
fn read_line(prompt: &str) -> Result<String, String> {
    let line = if std::io::stdin().is_terminal() {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        read_hidden()?
    } else {
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).map_err(|e| format!("reading stdin: {e}"))?;
        buf
    };
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// A line from the terminal with the echo turned off. The settings are put back afterwards.
#[cfg(unix)]
fn read_hidden() -> Result<String, String> {
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
fn read_hidden() -> Result<String, String> {
    Err("a hidden prompt needs a Unix terminal here: pipe the value on stdin".into())
}

/// `--nodes`: node ids (`n` and up to six digits, as the hub takes them), without repeats.
pub fn parse_nodes(values: &[String]) -> Result<Vec<String>, String> {
    let mut nodes: Vec<String> = Vec::new();
    for value in values {
        for node in value.split(',') {
            let node = node.trim();
            if !valid_node(node) {
                return Err(format!("{node} is not a node id: n followed by digits, such as n0"));
            }
            if !nodes.iter().any(|n| n == node) {
                nodes.push(node.to_string());
            }
        }
    }
    if nodes.is_empty() {
        return Err("--nodes names at least one node".into());
    }
    Ok(nodes)
}

/// `--bind`: bindings (`provider:`, `connector:`, `mcp:`, `tool:` with an id, or `hf`), sorted and
/// without repeats. Comma-separated values are split.
pub fn parse_bindings(values: &[String]) -> Result<Vec<String>, String> {
    let mut bindings = Vec::new();
    for value in values {
        for binding in value.split(',') {
            let binding = binding.trim();
            if !valid_binding(binding) {
                return Err(format!("{binding} is not a binding (provider:, connector:, mcp:, tool: with an id, or hf)"));
            }
            bindings.push(binding.to_string());
        }
    }
    bindings.sort();
    bindings.dedup();
    Ok(bindings)
}

/// `--bind NAME=binding,binding` for `import`: the bindings of each key, by name.
pub fn parse_bind_map(values: &[String]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for value in values {
        let Some((name, list)) = value.split_once('=') else {
            return Err("--bind wants NAME=binding,binding (a value without =)".into());
        };
        if !valid_name(name) {
            return Err(format!("{name} is not a name: 2 to 64 upper case letters, digits or _, starting with a letter"));
        }
        let bindings = parse_bindings(&[list.to_string()])?;
        let entry = map.entry(name.to_string()).or_default();
        entry.extend(bindings);
        entry.sort();
        entry.dedup();
    }
    Ok(map)
}

/// The scope as the hub takes it: every node, or the nodes named.
fn scope_value(nodes: Option<&[String]>) -> Result<Value, String> {
    match nodes {
        None => Ok(json!({ "kind": "plaza" })),
        Some(list) => Ok(json!({ "kind": "nodes", "nodes": parse_nodes(list)? })),
    }
}

/// The hub's name rule: `[A-Z][A-Z0-9_]{1,63}`.
fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=64).contains(&bytes.len())
        && bytes[0].is_ascii_uppercase()
        && bytes.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

/// The hub's node rule: `n` and one to six digits.
fn valid_node(node: &str) -> bool {
    let digits = node.strip_prefix('n').unwrap_or("");
    !digits.is_empty() && digits.len() <= 6 && digits.bytes().all(|c| c.is_ascii_digit())
}

/// The hub's binding rule: `hf`, or a known prefix and an id of lower case letters, digits, `.`,
/// `_` or `-`.
fn valid_binding(binding: &str) -> bool {
    let id_ok = |id: &str| {
        let bytes = id.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 64
            && bytes[0].is_ascii_alphanumeric()
            && bytes.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
    };
    binding == "hf" || binding.split_once(':').is_some_and(|(prefix, id)| matches!(prefix, "provider" | "connector" | "mcp" | "tool") && id_ok(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The vault's subcommands under a bare parser, so the argument rules can be tested.
    #[derive(Parser, Debug)]
    struct Harness {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["vault"];
        argv.extend_from_slice(args);
        Harness::try_parse_from(argv).map(|h| h.cmd)
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn every_subcommand_parses() {
        assert!(matches!(parse(&["status"]), Ok(Cmd::Status)));
        assert!(matches!(parse(&["unseal"]), Ok(Cmd::Unseal { recovery: false, keyfile: false })));
        assert!(matches!(parse(&["unseal", "--recovery"]), Ok(Cmd::Unseal { recovery: true, keyfile: false })));
        assert!(matches!(parse(&["unseal", "--keyfile"]), Ok(Cmd::Unseal { recovery: false, keyfile: true })));
        assert!(matches!(parse(&["seal"]), Ok(Cmd::Seal)));
        assert!(matches!(parse(&["list"]), Ok(Cmd::List)));
        assert!(matches!(parse(&["audit"]), Ok(Cmd::Audit { verify: false, after: 0, limit: 100 })));
        assert!(matches!(parse(&["audit", "--verify", "--after", "7", "--limit", "20"]), Ok(Cmd::Audit { verify: true, after: 7, limit: 20 })));
        assert!(matches!(parse(&["rekey"]), Ok(Cmd::Rekey)));
        assert!(matches!(parse(&["recovery"]), Ok(Cmd::Recovery)));
        assert!(matches!(parse(&["rm", "sec_abc"]), Ok(Cmd::Rm { yes: false, .. })));
        assert!(matches!(parse(&["rm", "sec_abc", "--yes"]), Ok(Cmd::Rm { yes: true, .. })));
        assert!(matches!(parse(&["import", "keys.env"]), Ok(Cmd::Import { shred: false, .. })));
    }

    #[test]
    fn set_takes_a_name_and_defaults_to_a_plaza_api_key() {
        match parse(&["set", "ANTHROPIC_API_KEY"]) {
            Ok(Cmd::Set { name, kind, label, scope, nodes, bind }) => {
                assert_eq!(name, "ANTHROPIC_API_KEY");
                assert_eq!(kind, Kind::ApiKey);
                assert_eq!(label, "");
                assert!(scope.is_none() && nodes.is_none());
                assert!(bind.is_empty());
            }
            other => panic!("set did not parse: {other:?}"),
        }
    }

    #[test]
    fn set_takes_its_options() {
        match parse(&["set", "GITHUB_TOKEN", "--kind", "token", "--label", "ci", "--scope", "plaza"]) {
            Ok(Cmd::Set { kind, label, scope, .. }) => {
                assert_eq!(kind, Kind::Token);
                assert_eq!(label, "ci");
                assert_eq!(scope.as_deref(), Some("plaza"));
            }
            other => panic!("set with options did not parse: {other:?}"),
        }
        match parse(&["set", "OTHER_KEY", "--nodes", "n0,n3"]) {
            Ok(Cmd::Set { nodes, .. }) => assert_eq!(nodes, Some(strings(&["n0", "n3"]))),
            other => panic!("set with nodes did not parse: {other:?}"),
        }
    }

    #[test]
    fn set_rejects_a_value_given_as_an_argument() {
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "--value", "x"]).is_err());
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "sk-ant-secret"]).is_err());
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "--value=x"]).is_err());
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "--passphrase", "x"]).is_err());
    }

    #[test]
    fn no_flag_accepts_a_secret_value() {
        assert!(parse(&["unseal", "--passphrase", "x"]).is_err());
        assert!(parse(&["unseal", "--recovery-code", "x"]).is_err());
        assert!(parse(&["unseal", "--recovery", "--keyfile"]).is_err());
        assert!(parse(&["rekey", "--secret", "x"]).is_err());
        assert!(parse(&["recovery", "--value", "x"]).is_err());
        assert!(parse(&["import", "keys.env", "--value", "x"]).is_err());
        assert!(parse(&["rm", "sec_abc", "--value", "x"]).is_err());
    }

    #[test]
    fn set_scope_and_nodes_are_one_choice_and_scope_only_says_plaza() {
        assert!(parse(&["set", "OTHER_KEY", "--scope", "plaza", "--nodes", "n0"]).is_err());
        assert!(parse(&["set", "OTHER_KEY", "--scope", "nodes"]).is_err());
        assert!(parse(&["set", "OTHER_KEY", "--kind", "bogus"]).is_err());
    }

    #[test]
    fn bind_repeats_and_splits_on_commas() {
        match parse(&["set", "KEY", "--bind", "provider:anthropic", "--bind", "provider:openai,hf"]) {
            Ok(Cmd::Set { bind, .. }) => assert_eq!(bind, strings(&["provider:anthropic", "provider:openai", "hf"])),
            other => panic!("repeated bind did not parse: {other:?}"),
        }
    }

    #[test]
    fn import_takes_nodes_and_name_bindings_and_shred() {
        match parse(&["import", "keys.env", "--nodes", "n1,n2", "--bind", "ANTHROPIC_API_KEY=provider:anthropic", "--bind", "SLACK=connector:slack,hf", "--shred"]) {
            Ok(Cmd::Import { file, nodes, bind, shred }) => {
                assert_eq!(file, PathBuf::from("keys.env"));
                assert_eq!(nodes, Some(strings(&["n1", "n2"])));
                assert_eq!(bind, strings(&["ANTHROPIC_API_KEY=provider:anthropic", "SLACK=connector:slack,hf"]));
                assert!(shred);
            }
            other => panic!("import did not parse: {other:?}"),
        }
    }

    #[test]
    fn nodes_are_n_and_digits_without_repeats() {
        assert_eq!(parse_nodes(&strings(&["n0,n3", "n3"])).unwrap(), strings(&["n0", "n3"]));
        assert_eq!(parse_nodes(&strings(&["n12"])).unwrap(), strings(&["n12"]));
        assert!(parse_nodes(&strings(&["x1"])).is_err());
        assert!(parse_nodes(&strings(&["n"])).is_err());
        assert!(parse_nodes(&strings(&["n1a"])).is_err());
        assert!(parse_nodes(&strings(&["n1234567"])).is_err());
        assert!(parse_nodes(&strings(&["n0,,n1"])).is_err());
        assert!(parse_nodes(&[]).is_err());
    }

    #[test]
    fn bindings_are_checked_sorted_and_split() {
        assert_eq!(
            parse_bindings(&strings(&["provider:openai,hf", "connector:slack", "tool:web-search", "mcp:github", "hf"])).unwrap(),
            strings(&["connector:slack", "hf", "mcp:github", "provider:openai", "tool:web-search"])
        );
        assert!(parse_bindings(&strings(&["bogus:x"])).is_err());
        assert!(parse_bindings(&strings(&["provider:"])).is_err());
        assert!(parse_bindings(&strings(&["provider:Anthropic"])).is_err());
        assert!(parse_bindings(&strings(&["provider:a,,hf"])).is_err());
    }

    #[test]
    fn bind_map_maps_each_name_to_its_bindings() {
        let map = parse_bind_map(&strings(&["SLACK_TOKEN=connector:slack,hf", "SLACK_TOKEN=connector:slack"])).unwrap();
        assert_eq!(map.get("SLACK_TOKEN"), Some(&strings(&["connector:slack", "hf"])));
        assert!(parse_bind_map(&strings(&["lower=provider:anthropic"])).is_err());
        assert!(parse_bind_map(&strings(&["NO_EQUALS"])).is_err());
        assert!(parse_bind_map(&strings(&["KEY=bogus:x"])).is_err());
    }

    #[test]
    fn names_follow_the_hub_rule() {
        assert!(valid_name("ANTHROPIC_API_KEY"));
        assert!(valid_name("K1"));
        assert!(!valid_name("A"));
        assert!(!valid_name("lower"));
        assert!(!valid_name("1KEY"));
        assert!(!valid_name("KEY-NAME"));
        assert!(!valid_name(&"A".repeat(65)));
    }

    #[test]
    fn scope_value_is_plaza_or_the_nodes() {
        assert_eq!(scope_value(None).unwrap(), json!({ "kind": "plaza" }));
        assert_eq!(scope_value(Some(&strings(&["n0", "n3"]))).unwrap(), json!({ "kind": "nodes", "nodes": ["n0", "n3"] }));
    }

    #[test]
    fn the_shred_decision_needs_every_key_and_no_skips() {
        let keys: BTreeSet<String> = ["A", "B"].iter().map(|s| s.to_string()).collect();
        let all: BTreeSet<String> = ["A", "B"].iter().map(|s| s.to_string()).collect();
        let some: BTreeSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        assert!(shred_verdict(&keys, &all, 0).is_ok());
        assert!(shred_verdict(&keys, &all, 1).is_err(), "a skipped key blocks the shred");
        assert!(shred_verdict(&keys, &some, 0).is_err(), "an unconfirmed key blocks the shred");
    }

    #[test]
    fn the_shred_decision_counts_the_keys_of_a_file() {
        let text = "# keys\nA=1\nexport B=\"two\nlines\"\nC=3\nA=4\n";
        let names = files::dotenv_names(text).unwrap();
        let distinct: BTreeSet<String> = names.into_iter().collect();
        assert_eq!(distinct.len(), 3, "A, B and C");
        let imported: BTreeSet<String> = distinct.iter().cloned().collect();
        assert!(shred_verdict(&distinct, &imported, 0).is_ok());
    }
}
