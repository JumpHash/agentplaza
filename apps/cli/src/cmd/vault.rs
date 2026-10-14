//! `agentplaza vault`: the hub's encrypted vault. A value never comes from the command line: `set`,
//! `unseal` and `rekey` read it from stdin when stdin is not a terminal, else from a prompt with no
//! echo. `import` moves a `.env` file's keys into the vault; `--shred` then removes the file, once
//! the hub has every key.

mod files;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

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
    /// A new value for an existing secret: a new version (the old ones stay for rollback). Its value
    /// is read from stdin (when it is not a terminal) or a hidden prompt.
    Rotate {
        /// The entry's name, or its id (sec_...) when more than one entry holds the name.
        name: String,
    },
    /// Make an earlier version the current one. Without --version, the newest one before the current.
    #[command(disable_version_flag = true)]
    Rollback {
        /// The entry's name, or its id (sec_...).
        name: String,
        /// The version to return to (see `list` for the versions an entry keeps).
        #[arg(long)]
        version: Option<u64>,
    },
    /// Change a secret's label, scope or bindings. --bind adds bindings and --unbind removes them.
    Edit {
        /// The entry's name, or its id (sec_...).
        name: String,
        /// A new label (an empty one clears it).
        #[arg(long)]
        label: Option<String>,
        /// Every node of the plaza. `plaza` is the only word.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["plaza"]), conflicts_with = "nodes")]
        scope: Option<String>,
        /// Only these nodes, e.g. n0,n3.
        #[arg(long, value_delimiter = ',', conflicts_with = "scope")]
        nodes: Option<Vec<String>>,
        /// A binding to add. Repeatable, or comma-separated.
        #[arg(long = "bind", value_delimiter = ',')]
        bind: Vec<String>,
        /// A binding to remove. Repeatable, or comma-separated.
        #[arg(long = "unbind", value_delimiter = ',')]
        unbind: Vec<String>,
    },
    /// Check a secret the way its bound consumer would, on a node. Prints whether it passed and the
    /// consumer's detail; no value is shown.
    Test {
        /// The entry's name, or its id (sec_...).
        name: String,
        /// The node to check on, e.g. n0 (default: this machine).
        #[arg(long)]
        node: Option<String>,
    },
    /// Delete an entry by its id (see `list`). A .env import will not bring its name back.
    Rm {
        /// The entry's id, e.g. sec_...
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// The consumers that read the vault (the providers, connectors and tools) and the entry each
    /// node resolves to: an id, `missing`, or `out_of_scope`.
    Consumers,
    /// The key slots: a passphrase or the key file. The vault keeps at least one.
    Slot {
        #[command(subcommand)]
        cmd: SlotCmd,
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

#[derive(Subcommand, Debug)]
pub enum SlotCmd {
    /// Add a slot. A passphrase slot asks for the new passphrase. A key file slot is made by the hub,
    /// in its own state directory (there is no path to give); the vault has one key file slot at most.
    Add {
        /// A passphrase slot: asks for the passphrase.
        #[arg(long, conflicts_with = "keyfile")]
        passphrase: bool,
        /// A key file slot: the hub creates the key file.
        #[arg(long, conflicts_with = "passphrase")]
        keyfile: bool,
    },
    /// Remove a slot by its id (see `status`). The last slot stays. Removing the key file slot
    /// destroys the key file.
    Remove {
        /// The slot's id, e.g. slt_...
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
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
                ctx::table(
                    &[
                        "id",
                        "name",
                        "kind",
                        "scope",
                        "bindings",
                        "version",
                        "uses",
                        "last used",
                    ],
                    &rows,
                );
            });
        }
        Cmd::Audit {
            verify,
            after,
            limit,
        } => {
            let mut path = format!("/v1/vault/audit?after={after}&limit={limit}");
            if verify {
                path.push_str("&verify=1");
            }
            let v = ctx.client.get(&path)?;
            let chain_ok = v.get("chain_ok").cloned().unwrap_or(Value::Null);
            ctx.out(&v, print_audit);
            if chain_ok == Value::Bool(false) {
                return Err(
                    "the audit chain is broken: a line was changed or removed after it was written"
                        .into(),
                );
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
                let what = if kind == "recovery" {
                    "recovery code"
                } else {
                    "passphrase"
                };
                let value = read_line(&format!("{what} of {id} (Enter if it is not needed): "))?;
                if !value.is_empty() {
                    secrets.insert(id.to_string(), Value::String(value));
                }
            }
            let v = ctx
                .client
                .post("/v1/vault/rekey", &json!({ "secrets": secrets }))?;
            ctx.out(&v, print_status);
        }
        Cmd::Recovery => {
            let v = ctx.client.post("/v1/vault/recovery", &json!({}))?;
            ctx.out(&v, |v| {
                eprintln!("the recovery code is shown once: keep it somewhere safe");
                println!("{}", ctx::s(v, "recovery_code"));
            });
        }
        Cmd::Set {
            name,
            kind,
            label,
            scope,
            nodes,
            bind,
        } => {
            if !valid_name(&name) {
                return Err(format!(
                    "{name} is not a name: 2 to 64 upper case letters, digits or _, starting with a letter"
                ));
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
            let v = match post_status(ctx, "/v1/vault/secrets", &body) {
                Ok(v) => v,
                Err((409, message)) => {
                    return Err(format!(
                        "{message}\nhint: to give {name} a new value, use `agentplaza vault rotate {name}`"
                    ));
                }
                Err((_, message)) => return Err(message),
            };
            ctx.out(&v, |v| {
                println!(
                    "stored {} as {} ({})",
                    ctx::s(v, "name"),
                    ctx::s(v, "id"),
                    scope_text(&v["scope"])
                )
            });
        }
        Cmd::Rotate { name } => {
            let entry = find_entry(ctx, &name)?;
            let value = secret(&format!("new value for {name}: "))?;
            let path = format!("/v1/vault/secrets/{}/value", ctx::seg(ctx::s(&entry, "id")));
            let v = ctx.client.put(&path, &json!({ "value": value }))?;
            ctx.out(&v, |v| {
                println!(
                    "rotated {} to v{}",
                    ctx::s(v, "name"),
                    ctx::text(v, "current")
                )
            });
        }
        Cmd::Rollback { name, version } => {
            let entry = find_entry(ctx, &name)?;
            let target = rollback_target(&entry, version)?;
            let path = format!(
                "/v1/vault/secrets/{}/rollback",
                ctx::seg(ctx::s(&entry, "id"))
            );
            let v = ctx.client.post(&path, &json!({ "v": target }))?;
            ctx.out(&v, |v| {
                println!(
                    "{} is back on v{}",
                    ctx::s(v, "name"),
                    ctx::text(v, "current")
                )
            });
        }
        Cmd::Edit {
            name,
            label,
            scope,
            nodes,
            bind,
            unbind,
        } => {
            let entry = find_entry(ctx, &name)?;
            let scope = if scope.is_some() || nodes.is_some() {
                Some(scope_value(nodes.as_deref())?)
            } else {
                None
            };
            let bindings = if bind.is_empty() && unbind.is_empty() {
                None
            } else {
                Some(merged_bindings(
                    &current_bindings(&entry),
                    &parse_bindings(&bind)?,
                    &parse_bindings(&unbind)?,
                )?)
            };
            let body = edit_body(label.as_deref(), scope, bindings.as_deref())?;
            let path = format!("/v1/vault/secrets/{}", ctx::seg(ctx::s(&entry, "id")));
            let v = ctx.client.patch(&path, &body)?;
            ctx.out(&v, |v| {
                let bound: Vec<&str> = array(&v["bindings"])
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                let bound = if bound.is_empty() {
                    "none".to_string()
                } else {
                    bound.join(",")
                };
                println!(
                    "saved {} (scope {}, bindings {bound})",
                    ctx::s(v, "name"),
                    scope_text(&v["scope"])
                );
            });
        }
        Cmd::Test { name, node } => {
            if let Some(node) = &node {
                parse_nodes(std::slice::from_ref(node))?;
            }
            let entry = find_entry(ctx, &name)?;
            let path = format!("/v1/vault/secrets/{}/test", ctx::seg(ctx::s(&entry, "id")));
            let v = ctx.client.post(&path, &test_body(node.as_deref()))?;
            ctx.out(&v, |v| {
                ctx::fields(&[
                    ("ok", ctx::text(v, "ok")),
                    ("detail", ctx::text(v, "detail")),
                ])
            });
            if v["ok"] != json!(true) {
                return Err(format!("the check of {} failed", ctx::s(&entry, "name")));
            }
        }
        Cmd::Consumers => {
            let v = ctx.client.get("/v1/vault/consumers")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v).iter().map(consumer_row).collect();
                ctx::table(&["binding", "label", "wants", "per node"], &rows);
            });
        }
        Cmd::Slot {
            cmd: SlotCmd::Add {
                passphrase,
                keyfile,
            },
        } => {
            let body = match (passphrase, keyfile) {
                (true, false) => slot_body(Some(&secret("passphrase for the new slot: ")?)),
                (false, true) => slot_body(None),
                _ => return Err("give one kind of slot: --passphrase or --keyfile".into()),
            };
            let v = ctx.client.post("/v1/vault/slots", &body)?;
            ctx.out(&v, print_status);
        }
        Cmd::Slot {
            cmd: SlotCmd::Remove { id, yes },
        } => {
            if !confirm(
                yes,
                &format!("Remove slot {id}? Removing the key file slot destroys the key file."),
            )? {
                println!("cancelled");
                return Ok(());
            }
            let v = ctx
                .client
                .delete(&format!("/v1/vault/slots/{}", ctx::seg(&id)))?;
            ctx.out(&v, print_status);
        }
        Cmd::Rm { id, yes } => {
            if !confirm(
                yes,
                &format!(
                    "Delete entry {id}? Its value is gone, and a .env import will not bring it back."
                ),
            )? {
                println!("cancelled");
                return Ok(());
            }
            let v = ctx
                .client
                .delete(&format!("/v1/vault/secrets/{}", ctx::seg(&id)))?;
            ctx.out(&v, |_| println!("removed {id}"));
        }
        Cmd::Import {
            file,
            nodes,
            bind,
            shred,
        } => import(ctx, &file, nodes.as_deref(), &bind, shred)?,
    }
    Ok(())
}

/// `agentplaza vault import`: the warning, then the file's keys to the hub; `--shred` removes the
/// file only after every key is in the vault.
fn import(
    ctx: &Ctx,
    file: &Path,
    nodes: Option<&[String]>,
    bind: &[String],
    shred: bool,
) -> Result<(), String> {
    eprintln!("{IMPORT_WARNING}");
    let scope = scope_value(nodes)?;
    let bindings = parse_bind_map(bind)?;
    let left_alone = |e: String| {
        if shred {
            format!("{e}; {} was left alone", file.display())
        } else {
            e
        }
    };
    if shred {
        files::check_shreddable(file).map_err(left_alone)?;
    }
    let text = std::fs::read_to_string(file)
        .map_err(|e| left_alone(format!("{}: {e}", file.display())))?;
    let keys: BTreeSet<String> = files::dotenv_names(&text)
        .map_err(|e| left_alone(format!("{}: {e}", file.display())))?
        .into_iter()
        .collect();
    let body = json!({ "format": "dotenv", "text": text, "scope": scope, "bindings": bindings });
    let v = ctx
        .client
        .post("/v1/vault/import", &body)
        .map_err(left_alone)?;
    ctx.out(&v, print_import);
    if !shred {
        return Ok(());
    }
    let imported = imported_names(&v);
    let skipped = array(&v["skipped"]).len();
    if let Err(why) = shred_verdict(&keys, &imported, skipped) {
        return Err(format!(
            "not shredding {}: {why}; the file was left alone",
            file.display()
        ));
    }
    files::shred(file).map_err(|e| format!("imported, but shredding failed: {e}"))?;
    println!("shredded {}", file.display());
    Ok(())
}

/// Whether `--shred` may remove the file: the hub skipped nothing, and it confirmed every distinct
/// key of the file. Otherwise the reason.
fn shred_verdict(
    keys: &BTreeSet<String>,
    imported: &BTreeSet<String>,
    skipped: usize,
) -> Result<(), String> {
    if skipped > 0 {
        return Err(format!("{skipped} key(s) were skipped"));
    }
    match keys.iter().find(|key| !imported.contains(*key)) {
        Some(key) => Err(format!("the hub did not confirm {key}")),
        None => Ok(()),
    }
}

fn imported_names(v: &Value) -> BTreeSet<String> {
    array(&v["imported"])
        .iter()
        .map(|e| ctx::s(e, "name").to_string())
        .collect()
}

fn print_import(v: &Value) {
    let names: Vec<&str> = array(&v["imported"])
        .iter()
        .map(|e| ctx::s(e, "name"))
        .collect();
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
    let slots: Vec<String> = array(&v["slots"])
        .iter()
        .map(|s| format!("{} ({})", ctx::s(s, "id"), ctx::s(s, "kind")))
        .collect();
    let nodes: Vec<String> = array(&v["nodes"])
        .iter()
        .map(|n| format!("{} ({})", ctx::s(n, "name"), ctx::s(n, "id")))
        .collect();
    let mut pairs = vec![
        ("state", ctx::s(v, "state").to_string()),
        ("entries", ctx::text(v, "entries")),
        (
            "slots",
            if slots.is_empty() {
                "none".to_string()
            } else {
                slots.join(", ")
            },
        ),
        ("nodes", nodes.join(", ")),
        ("audit", audit_label(v).to_string()),
    ];
    if !v["env_file_keys"].is_null() {
        pairs.push(("env file keys", ctx::text(v, "env_file_keys")));
    }
    if !v["rekeyed_ms"].is_null() {
        pairs.push(("rekeyed", ctx::when(&v["rekeyed_ms"])));
    }
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
    ctx::table(
        &[
            "seq", "at", "event", "actor", "name", "binding", "node", "detail",
        ],
        &rows,
    );
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
    let bindings: Vec<&str> = array(&e["bindings"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let last_used = if e["last_used_ms"].as_u64().unwrap_or(0) == 0 {
        "never".to_string()
    } else {
        ctx::ago(&e["last_used_ms"])
    };
    vec![
        ctx::s(e, "id").to_string(),
        ctx::s(e, "name").to_string(),
        ctx::s(e, "kind").to_string(),
        scope_text(&e["scope"]),
        if bindings.is_empty() {
            "—".to_string()
        } else {
            bindings.join(",")
        },
        format!("v{current} {fingerprint}"),
        ctx::text(e, "use_count"),
        last_used,
    ]
}

/// A POST whose refusal keeps its HTTP status: (status, the hub's `error` text). The shared client
/// drops the status, and `set` needs it to tell a name that is taken (409) from other refusals.
fn post_status(ctx: &Ctx, path: &str, body: &Value) -> Result<Value, (u16, String)> {
    let (status, value) = apz_http::client::request_json(
        &ctx.client.target,
        "POST",
        path,
        Some(body),
        ctx.client.timeout,
    )
    .map_err(|e| (0, format!("{} {path}: {e}", ctx.client.base())))?;
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let message = value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("POST {path}: HTTP {status}"));
    Err((status, message))
}

/// The entry a command names: looked up in the vault's list (see [`pick_entry`]).
fn find_entry(ctx: &Ctx, arg: &str) -> Result<Value, String> {
    let list = ctx.client.get("/v1/vault/secrets")?;
    pick_entry(array(&list), arg).cloned()
}

/// The entry `arg` names: by id when it starts with `sec_`, else by name. A name that two or more
/// entries hold (one per node set) is refused with their ids, so the caller can pick one.
fn pick_entry<'a>(entries: &'a [Value], arg: &str) -> Result<&'a Value, String> {
    let by_id = arg.starts_with("sec_");
    let found: Vec<&Value> = entries
        .iter()
        .filter(|e| ctx::s(e, if by_id { "id" } else { "name" }) == arg)
        .collect();
    match found.as_slice() {
        [one] => Ok(*one),
        [] if by_id => Err(format!("no secret has the id {arg}")),
        [] => Err(format!("no secret is called {arg}")),
        many => {
            let ids: Vec<&str> = many.iter().map(|e| ctx::s(e, "id")).collect();
            Err(format!(
                "{arg} is held by {} entries ({}): name one by its id",
                many.len(),
                ids.join(", ")
            ))
        }
    }
}

/// The version a rollback returns to: the one asked for (it must be kept, and not be the current
/// one), else the newest version before the current one.
fn rollback_target(entry: &Value, asked: Option<u64>) -> Result<u64, String> {
    let name = ctx::s(entry, "name");
    let current = entry["current"].as_u64().unwrap_or(0);
    let kept: Vec<u64> = array(&entry["versions"])
        .iter()
        .filter_map(|v| v["v"].as_u64())
        .collect();
    match asked {
        Some(v) if !kept.contains(&v) => {
            let listed: Vec<String> = kept.iter().map(|k| format!("v{k}")).collect();
            let listed = if listed.is_empty() {
                "none".to_string()
            } else {
                listed.join(", ")
            };
            Err(format!("{name} keeps no v{v} (it keeps {listed})"))
        }
        Some(v) if v == current => Err(format!("v{v} is already the current version of {name}")),
        Some(v) => Ok(v),
        None => kept
            .iter()
            .copied()
            .filter(|v| *v < current)
            .max()
            .ok_or_else(|| format!("{name} has no earlier version to return to")),
    }
}

/// The bindings an entry has now, as the hub lists them.
fn current_bindings(entry: &Value) -> Vec<String> {
    array(&entry["bindings"])
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The bindings after an edit: the current ones without `unbind`, plus `bind`; sorted, no repeats.
/// Removing a binding the entry does not have is refused, so a typo does not pass silently.
fn merged_bindings(
    current: &[String],
    bind: &[String],
    unbind: &[String],
) -> Result<Vec<String>, String> {
    if let Some(missing) = unbind.iter().find(|b| !current.contains(b)) {
        return Err(format!("{missing} is not one of its bindings"));
    }
    let mut out: Vec<String> = current
        .iter()
        .filter(|b| !unbind.contains(b))
        .cloned()
        .collect();
    out.extend(bind.iter().cloned());
    out.sort();
    out.dedup();
    Ok(out)
}

/// The PATCH body of `edit`: only the fields given. Refused when none is.
fn edit_body(
    label: Option<&str>,
    scope: Option<Value>,
    bindings: Option<&[String]>,
) -> Result<Value, String> {
    let mut body = serde_json::Map::new();
    if let Some(label) = label {
        body.insert("label".into(), json!(label));
    }
    if let Some(scope) = scope {
        body.insert("scope".into(), scope);
    }
    if let Some(bindings) = bindings {
        body.insert("bindings".into(), json!(bindings));
    }
    if body.is_empty() {
        return Err("nothing to change: give --label, --scope, --nodes, --bind or --unbind".into());
    }
    Ok(Value::Object(body))
}

/// The body of `test`: the node to check on, or none (this machine).
fn test_body(node: Option<&str>) -> Value {
    json!({ "node": node })
}

/// The body of `slot add`: a passphrase slot carries the passphrase; a key file slot carries nothing,
/// since the hub makes the key file itself.
fn slot_body(passphrase: Option<&str>) -> Value {
    match passphrase {
        Some(passphrase) => json!({ "kind": "passphrase", "passphrase": passphrase }),
        None => json!({ "kind": "keyfile" }),
    }
}

/// One row of `consumers`: the binding, its label, the name it wants, and each node's entry.
fn consumer_row(c: &Value) -> Vec<String> {
    let per_node: Vec<String> = c["per_node"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(node, state)| format!("{node}: {}", state.as_str().unwrap_or("?")))
                .collect()
        })
        .unwrap_or_default();
    vec![
        ctx::s(c, "binding").to_string(),
        ctx::s(c, "label").to_string(),
        ctx::s(c, "wants").to_string(),
        if per_node.is_empty() {
            "—".to_string()
        } else {
            per_node.join(", ")
        },
    ]
}

/// The audit's state as the status shows it: `audit_ok` true, false, or null when it was not checked.
fn audit_label(v: &Value) -> &'static str {
    match v.get("audit_ok") {
        Some(Value::Bool(true)) => "ok",
        Some(Value::Bool(false)) => "BROKEN",
        Some(Value::Null) => "not checked",
        _ => "unknown",
    }
}

/// A scope as the people read it: `plaza`, or `nodes n0,n3`.
fn scope_text(scope: &Value) -> String {
    if ctx::s(scope, "kind") == "nodes" {
        let nodes: Vec<&str> = array(&scope["nodes"])
            .iter()
            .filter_map(Value::as_str)
            .collect();
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
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
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
                return Err(format!(
                    "{node} is not a node id: n followed by digits, such as n0"
                ));
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
                return Err(format!(
                    "{binding} is not a binding (provider:, connector:, mcp:, tool: with an id, or hf)"
                ));
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
            return Err(format!(
                "{name} is not a name: 2 to 64 upper case letters, digits or _, starting with a letter"
            ));
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
        && bytes
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
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
            && bytes.iter().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
            })
    };
    binding == "hf"
        || binding.split_once(':').is_some_and(|(prefix, id)| {
            matches!(prefix, "provider" | "connector" | "mcp" | "tool") && id_ok(id)
        })
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
        assert!(matches!(
            parse(&["unseal"]),
            Ok(Cmd::Unseal {
                recovery: false,
                keyfile: false
            })
        ));
        assert!(matches!(
            parse(&["unseal", "--recovery"]),
            Ok(Cmd::Unseal {
                recovery: true,
                keyfile: false
            })
        ));
        assert!(matches!(
            parse(&["unseal", "--keyfile"]),
            Ok(Cmd::Unseal {
                recovery: false,
                keyfile: true
            })
        ));
        assert!(matches!(parse(&["seal"]), Ok(Cmd::Seal)));
        assert!(matches!(parse(&["list"]), Ok(Cmd::List)));
        assert!(matches!(
            parse(&["audit"]),
            Ok(Cmd::Audit {
                verify: false,
                after: 0,
                limit: 100
            })
        ));
        assert!(matches!(
            parse(&["audit", "--verify", "--after", "7", "--limit", "20"]),
            Ok(Cmd::Audit {
                verify: true,
                after: 7,
                limit: 20
            })
        ));
        assert!(matches!(parse(&["rekey"]), Ok(Cmd::Rekey)));
        assert!(matches!(parse(&["recovery"]), Ok(Cmd::Recovery)));
        assert!(matches!(
            parse(&["rm", "sec_abc"]),
            Ok(Cmd::Rm { yes: false, .. })
        ));
        assert!(matches!(
            parse(&["rm", "sec_abc", "--yes"]),
            Ok(Cmd::Rm { yes: true, .. })
        ));
        assert!(matches!(
            parse(&["import", "keys.env"]),
            Ok(Cmd::Import { shred: false, .. })
        ));
    }

    #[test]
    fn set_takes_a_name_with_defaults_and_its_options() {
        match parse(&["set", "ANTHROPIC_API_KEY"]) {
            Ok(Cmd::Set {
                name,
                kind,
                label,
                scope,
                nodes,
                bind,
            }) => {
                assert_eq!(name, "ANTHROPIC_API_KEY");
                assert_eq!(kind, Kind::ApiKey, "a plaza API key by default");
                assert_eq!(label, "");
                assert!(scope.is_none() && nodes.is_none());
                assert!(bind.is_empty());
            }
            other => panic!("set did not parse: {other:?}"),
        }
        match parse(&[
            "set",
            "GITHUB_TOKEN",
            "--kind",
            "token",
            "--label",
            "ci",
            "--scope",
            "plaza",
        ]) {
            Ok(Cmd::Set {
                kind, label, scope, ..
            }) => {
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
    fn no_command_takes_a_secret_value_as_an_argument() {
        // A secret is typed at a prompt or read from stdin, never passed on the command line.
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "--value", "x"]).is_err());
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "sk-ant-secret"]).is_err());
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "--value=x"]).is_err());
        assert!(parse(&["set", "ANTHROPIC_API_KEY", "--passphrase", "x"]).is_err());
        assert!(parse(&["unseal", "--passphrase", "x"]).is_err());
        assert!(parse(&["unseal", "--recovery-code", "x"]).is_err());
        assert!(parse(&["unseal", "--recovery", "--keyfile"]).is_err());
        assert!(parse(&["rekey", "--secret", "x"]).is_err());
        assert!(parse(&["recovery", "--value", "x"]).is_err());
        assert!(parse(&["import", "keys.env", "--value", "x"]).is_err());
        assert!(parse(&["rm", "sec_abc", "--value", "x"]).is_err());
        assert!(parse(&["rotate", "ANTHROPIC_API_KEY", "sk-ant-secret"]).is_err());
        assert!(parse(&["rotate", "ANTHROPIC_API_KEY", "--value", "x"]).is_err());
        assert!(parse(&["edit", "KEY", "--value", "x"]).is_err());
        assert!(parse(&["slot", "add", "--passphrase", "hunter2"]).is_err());
        assert!(
            parse(&["slot", "add", "--keyfile", "/tmp/key"]).is_err(),
            "the hub makes the key file: no path"
        );
        assert!(
            parse(&["slot", "add", "--passphrase", "--keyfile"]).is_err(),
            "one kind of slot"
        );
        assert!(parse(&["edit", "KEY", "--scope", "plaza", "--nodes", "n0"]).is_err());
    }

    #[test]
    fn set_scope_and_nodes_are_one_choice_and_scope_only_says_plaza() {
        assert!(parse(&["set", "OTHER_KEY", "--scope", "plaza", "--nodes", "n0"]).is_err());
        assert!(parse(&["set", "OTHER_KEY", "--scope", "nodes"]).is_err());
        assert!(parse(&["set", "OTHER_KEY", "--kind", "bogus"]).is_err());
    }

    #[test]
    fn bind_repeats_and_splits_on_commas() {
        match parse(&[
            "set",
            "KEY",
            "--bind",
            "provider:anthropic",
            "--bind",
            "provider:openai,hf",
        ]) {
            Ok(Cmd::Set { bind, .. }) => assert_eq!(
                bind,
                strings(&["provider:anthropic", "provider:openai", "hf"])
            ),
            other => panic!("repeated bind did not parse: {other:?}"),
        }
    }

    #[test]
    fn import_takes_nodes_and_name_bindings_and_shred() {
        match parse(&[
            "import",
            "keys.env",
            "--nodes",
            "n1,n2",
            "--bind",
            "ANTHROPIC_API_KEY=provider:anthropic",
            "--bind",
            "SLACK=connector:slack,hf",
            "--shred",
        ]) {
            Ok(Cmd::Import {
                file,
                nodes,
                bind,
                shred,
            }) => {
                assert_eq!(file, PathBuf::from("keys.env"));
                assert_eq!(nodes, Some(strings(&["n1", "n2"])));
                assert_eq!(
                    bind,
                    strings(&[
                        "ANTHROPIC_API_KEY=provider:anthropic",
                        "SLACK=connector:slack,hf"
                    ])
                );
                assert!(shred);
            }
            other => panic!("import did not parse: {other:?}"),
        }
    }

    #[test]
    fn nodes_are_n_and_digits_without_repeats() {
        assert_eq!(
            parse_nodes(&strings(&["n0,n3", "n3"])).unwrap(),
            strings(&["n0", "n3"])
        );
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
            parse_bindings(&strings(&[
                "provider:openai,hf",
                "connector:slack",
                "tool:web-search",
                "mcp:github",
                "hf"
            ]))
            .unwrap(),
            strings(&[
                "connector:slack",
                "hf",
                "mcp:github",
                "provider:openai",
                "tool:web-search"
            ])
        );
        assert!(parse_bindings(&strings(&["bogus:x"])).is_err());
        assert!(parse_bindings(&strings(&["provider:"])).is_err());
        assert!(parse_bindings(&strings(&["provider:Anthropic"])).is_err());
        assert!(parse_bindings(&strings(&["provider:a,,hf"])).is_err());
    }

    #[test]
    fn bind_map_maps_each_name_to_its_bindings() {
        let map = parse_bind_map(&strings(&[
            "SLACK_TOKEN=connector:slack,hf",
            "SLACK_TOKEN=connector:slack",
        ]))
        .unwrap();
        assert_eq!(
            map.get("SLACK_TOKEN"),
            Some(&strings(&["connector:slack", "hf"]))
        );
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
        assert_eq!(
            scope_value(Some(&strings(&["n0", "n3"]))).unwrap(),
            json!({ "kind": "nodes", "nodes": ["n0", "n3"] })
        );
    }

    #[test]
    fn the_shred_decision_needs_every_key_and_no_skips() {
        let keys: BTreeSet<String> = ["A", "B"].iter().map(|s| s.to_string()).collect();
        let all: BTreeSet<String> = ["A", "B"].iter().map(|s| s.to_string()).collect();
        let some: BTreeSet<String> = ["A"].iter().map(|s| s.to_string()).collect();
        assert!(shred_verdict(&keys, &all, 0).is_ok());
        assert!(
            shred_verdict(&keys, &all, 1).is_err(),
            "a skipped key blocks the shred"
        );
        assert!(
            shred_verdict(&keys, &some, 0).is_err(),
            "an unconfirmed key blocks the shred"
        );
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

    #[test]
    fn the_new_commands_parse() {
        assert!(matches!(
            parse(&["rotate", "ANTHROPIC_API_KEY"]),
            Ok(Cmd::Rotate { .. })
        ));
        assert!(matches!(
            parse(&["rollback", "KEY"]),
            Ok(Cmd::Rollback { version: None, .. })
        ));
        assert!(matches!(
            parse(&["rollback", "KEY", "--version", "2"]),
            Ok(Cmd::Rollback {
                version: Some(2),
                ..
            })
        ));
        assert!(matches!(parse(&["edit", "KEY"]), Ok(Cmd::Edit { .. })));
        assert!(matches!(
            parse(&["test", "KEY"]),
            Ok(Cmd::Test { node: None, .. })
        ));
        assert!(matches!(
            parse(&["test", "KEY", "--node", "n3"]),
            Ok(Cmd::Test { node: Some(_), .. })
        ));
        assert!(matches!(parse(&["consumers"]), Ok(Cmd::Consumers)));
        assert!(matches!(
            parse(&["slot", "add", "--passphrase"]),
            Ok(Cmd::Slot {
                cmd: SlotCmd::Add {
                    passphrase: true,
                    keyfile: false
                }
            })
        ));
        assert!(matches!(
            parse(&["slot", "add", "--keyfile"]),
            Ok(Cmd::Slot {
                cmd: SlotCmd::Add {
                    passphrase: false,
                    keyfile: true
                }
            })
        ));
        assert!(matches!(
            parse(&["slot", "remove", "slt_abc", "--yes"]),
            Ok(Cmd::Slot {
                cmd: SlotCmd::Remove { yes: true, .. }
            })
        ));
    }

    #[test]
    fn an_entry_is_picked_by_name_or_by_id() {
        let entries = vec![
            json!({ "id": "sec_a", "name": "KEY", "scope": { "kind": "nodes", "nodes": ["n0"] } }),
            json!({ "id": "sec_b", "name": "KEY", "scope": { "kind": "nodes", "nodes": ["n3"] } }),
            json!({ "id": "sec_c", "name": "OTHER" }),
        ];
        assert_eq!(pick_entry(&entries, "OTHER").unwrap()["id"], "sec_c");
        assert_eq!(pick_entry(&entries, "sec_b").unwrap()["id"], "sec_b");
        assert!(
            pick_entry(&entries, "KEY")
                .unwrap_err()
                .contains("sec_a, sec_b"),
            "a shared name lists the ids"
        );
        assert!(pick_entry(&entries, "MISSING").is_err());
        assert!(pick_entry(&entries, "sec_zzz").is_err());
    }

    #[test]
    fn a_rollback_goes_to_the_asked_version_or_the_one_before_the_current() {
        let entry = json!({ "name": "KEY", "current": 3, "versions": [{ "v": 1 }, { "v": 2 }, { "v": 3 }] });
        assert_eq!(rollback_target(&entry, None).unwrap(), 2);
        assert_eq!(rollback_target(&entry, Some(1)).unwrap(), 1);
        assert!(rollback_target(&entry, Some(3)).is_err(), "already current");
        assert!(rollback_target(&entry, Some(9)).is_err(), "not kept");
        let first = json!({ "name": "KEY", "current": 1, "versions": [{ "v": 1 }] });
        assert!(
            rollback_target(&first, None).is_err(),
            "nothing before the first"
        );
    }

    #[test]
    fn an_edit_adds_and_removes_bindings_and_refuses_an_unknown_removal() {
        let current = strings(&["provider:anthropic", "hf"]);
        assert_eq!(
            merged_bindings(&current, &strings(&["connector:slack"]), &strings(&["hf"])).unwrap(),
            strings(&["connector:slack", "provider:anthropic"])
        );
        assert_eq!(
            merged_bindings(&current, &strings(&["hf"]), &[]).unwrap(),
            strings(&["hf", "provider:anthropic"])
        );
        assert!(merged_bindings(&current, &[], &strings(&["mcp:github"])).is_err());
    }

    #[test]
    fn request_bodies_carry_only_what_the_hub_takes() {
        assert_eq!(
            edit_body(Some("ci"), None, None).unwrap(),
            json!({ "label": "ci" })
        );
        assert_eq!(
            edit_body(None, Some(json!({ "kind": "plaza" })), None).unwrap(),
            json!({ "scope": { "kind": "plaza" } })
        );
        assert_eq!(
            edit_body(Some(""), None, Some(&strings(&["hf"]))).unwrap(),
            json!({ "label": "", "bindings": ["hf"] })
        );
        assert!(edit_body(None, None, None).is_err());
        assert_eq!(test_body(Some("n3")), json!({ "node": "n3" }));
        assert_eq!(test_body(None), json!({ "node": null }));
        assert_eq!(
            slot_body(Some("a passphrase")),
            json!({ "kind": "passphrase", "passphrase": "a passphrase" })
        );
        assert_eq!(slot_body(None), json!({ "kind": "keyfile" }));
    }

    #[test]
    fn a_consumer_row_names_each_node_entry() {
        let row = consumer_row(&json!({
            "binding": "provider:anthropic",
            "label": "Anthropic",
            "wants": "ANTHROPIC_API_KEY",
            "per_node": { "n0": "sec_a", "n3": "missing" },
        }));
        assert_eq!(
            row,
            strings(&[
                "provider:anthropic",
                "Anthropic",
                "ANTHROPIC_API_KEY",
                "n0: sec_a, n3: missing"
            ])
        );
    }

    #[test]
    fn a_null_audit_is_not_checked() {
        assert_eq!(audit_label(&json!({ "audit_ok": null })), "not checked");
        assert_eq!(audit_label(&json!({ "audit_ok": true })), "ok");
        assert_eq!(audit_label(&json!({ "audit_ok": false })), "BROKEN");
        assert_eq!(audit_label(&json!({})), "unknown");
    }
}
