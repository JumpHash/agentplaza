//! `agentplaza models`: Model providers, keys, policy, decision models.

use std::io::Read;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

/// The provider kinds whose models are decision models (as the console decides it).
const DECISION_KINDS: [&str; 3] = ["openai-decisions", "systemone", "azure-decisions"];
/// The effort levels the hub's settings accept, and `default`, which clears the setting (the provider's own default).
const EFFORTS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "default",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The configured providers: kind, endpoint, key status, models.
    Providers {
        /// Only the `chat` or the `decisions` providers.
        #[arg(long)]
        group: Option<String>,
    },
    /// The provider kinds you can add (presets).
    Presets,
    /// Add a provider from a preset (`add openai`), or a custom one (`add --id x --kind y`).
    Add {
        /// The preset to add; see `models presets`.
        preset: Option<String>,
        /// The provider id (default: the preset's id).
        #[arg(long)]
        id: Option<String>,
        /// The provider kind, for a custom provider (a preset brings its own).
        #[arg(long)]
        kind: Option<String>,
        /// The endpoint (default: the preset's).
        #[arg(long)]
        base_url: Option<String>,
        /// Auth from an environment variable of the hub's process.
        #[arg(long)]
        env: Option<String>,
        /// Auth from a named entry of the hub's secret store.
        #[arg(long)]
        secret: Option<String>,
        /// Auth from a shell command whose output is a bearer token.
        #[arg(long)]
        command: Option<String>,
        /// The models it offers, comma-separated (default: the preset's).
        #[arg(long)]
        models: Option<String>,
        /// Store the API key read from stdin with the provider.
        #[arg(long)]
        key_stdin: bool,
    },
    /// Change a provider's endpoint, auth or models. Flags not given keep what it has.
    Edit {
        /// The provider id.
        id: String,
        /// The new endpoint.
        #[arg(long)]
        base_url: Option<String>,
        /// Auth from an environment variable of the hub's process.
        #[arg(long)]
        env: Option<String>,
        /// Auth from a named entry of the hub's secret store.
        #[arg(long)]
        secret: Option<String>,
        /// Auth from a shell command whose output is a bearer token.
        #[arg(long)]
        command: Option<String>,
        /// The models it offers, comma-separated.
        #[arg(long)]
        models: Option<String>,
    },
    /// Remove a provider. Asks first unless --yes.
    Remove {
        /// The provider id.
        id: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Store a provider's API key. `--value -` reads it from stdin; it is never echoed.
    Key {
        /// The provider id.
        id: String,
        /// `-`: read the key from stdin.
        #[arg(long)]
        value: String,
    },
    /// Test a provider with one short call (to `--model`, else its first model).
    Test {
        /// The provider id.
        id: String,
        /// The model to call.
        #[arg(long)]
        model: Option<String>,
    },
    /// Every model the providers list.
    List {
        /// Only the `chat` or the `decisions` models.
        #[arg(long)]
        group: Option<String>,
    },
    /// The model policy and its limits.
    Policy {
        #[command(subcommand)]
        action: PolicyCmd,
    },
    /// The decision model the plaza asks.
    Decisions {
        #[command(subcommand)]
        action: DecisionsCmd,
    },
    /// One model's price, in USD per million tokens (`usage prices` lists them all).
    Price {
        /// The model, e.g. anthropic/claude-x.
        model: String,
    },
    /// The model prices, or the price sources refreshed.
    Prices {
        #[command(subcommand)]
        action: Option<PricesCmd>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PricesCmd {
    /// Fetches the price sources again now, then prints what each one returned.
    Refresh,
}

#[derive(Subcommand, Debug)]
pub enum PolicyCmd {
    /// The policy, the defaults, the limits, the prices.
    Show,
    /// Change the policy. A list given replaces the whole list; entries are provider/model, with an optional @effort.
    Set {
        /// The orchestrator tier's models, comma-separated.
        #[arg(long)]
        orchestrator: Option<String>,
        /// The worker tier's models, comma-separated.
        #[arg(long)]
        worker: Option<String>,
        /// The effort agents run at: none, minimal, low, medium, high, xhigh, max. `default` clears it (the provider's own default).
        #[arg(long)]
        effort: Option<String>,
        /// The most turns an agent takes.
        #[arg(long)]
        max_turns: Option<u64>,
        /// The most tokens an agent may spend per turn.
        #[arg(long)]
        max_tokens: Option<u64>,
        /// A host agents may not fetch from: example.com (and its subdomains), or *.example.com (subdomains only). Repeatable.
        #[arg(long = "fetch-block", value_name = "HOST")]
        fetch_block: Vec<String>,
        /// A host to take off the fetch blocklist. Repeatable.
        #[arg(long = "fetch-unblock", value_name = "HOST")]
        fetch_unblock: Vec<String>,
        /// The worker default model, provider/model.
        #[arg(long)]
        default_model: Option<String>,
        /// The orchestrator model, provider/model.
        #[arg(long)]
        orchestrator_model: Option<String>,
        /// One role's model, ROLE=provider/model (an empty model clears it). Repeatable.
        #[arg(long = "role-model", value_name = "ROLE=MODEL")]
        role_models: Vec<String>,
        /// One model's price in credits per 1M tokens, MODEL=INPUT:OUTPUT. Repeatable.
        #[arg(long = "price", value_name = "MODEL=IN:OUT")]
        prices: Vec<String>,
        /// Whether contributor models (their vendor trains on the data sent to them) may be defaults, role models and policy entries.
        #[arg(long, value_name = "true|false")]
        allow_training_models: Option<bool>,
        /// Minutes an agent idles before it sleeps (its VM snapshotted and stopped until work comes); 0 = never.
        #[arg(long, value_name = "MINUTES")]
        idle_sleep_min: Option<u32>,
        /// Roles whose agents never sleep, comma-separated (an empty value clears the list).
        #[arg(long, value_name = "ROLES")]
        sleep_never_roles: Option<String>,
        /// Whether the hub checkpoints an agent's whole VM before each irreversible tool call.
        #[arg(long, value_name = "true|false")]
        vm_auto_checkpoints: Option<bool>,
    },
}

#[derive(Subcommand, Debug)]
pub enum DecisionsCmd {
    /// The decision model. With a provider and a model, sets it.
    #[command(name = "default")]
    Current {
        /// The provider id.
        provider: Option<String>,
        /// The model id.
        model: Option<String>,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Providers { group } => {
            let want = group_filter(group)?;
            let list = ctx.client.get("/v1/providers")?;
            let kept = filter(&list, want.as_deref(), provider_group);
            ctx.out(&kept, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|p| {
                        vec![
                            ctx::s(p, "id").to_string(),
                            ctx::s(p, "kind").to_string(),
                            provider_group(p).to_string(),
                            key_status(ctx::s(p, "credential")).to_string(),
                            ctx::text(p, "base_url"),
                            p.get("models")
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len)
                                .to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "kind", "group", "key", "endpoint", "models"], &rows);
            });
            Ok(())
        }
        Cmd::Presets => {
            let kinds = ctx.client.get("/v1/provider-kinds")?;
            ctx.out(&kinds, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|k| {
                        vec![
                            ctx::s(k, "id").to_string(),
                            ctx::s(k, "label").to_string(),
                            ctx::s(k, "kind").to_string(),
                            ctx::text(k, "base_url"),
                            auth_label(k.get("auth")),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "label", "kind", "endpoint", "auth"], &rows);
            });
            Ok(())
        }
        Cmd::Add {
            preset,
            id,
            kind,
            base_url,
            env,
            secret,
            command,
            models,
            key_stdin,
        } => {
            let key = if key_stdin {
                Some(read_secret_from_stdin()?)
            } else {
                None
            };
            let mut body = match &preset {
                Some(name) => {
                    if kind.is_some() {
                        return Err("a preset brings its own kind: drop --kind".into());
                    }
                    let kinds = ctx.client.get("/v1/provider-kinds")?;
                    let found = items(&kinds)
                        .find(|k| ctx::s(k, "id") == name.as_str())
                        .cloned()
                        .ok_or_else(|| {
                            format!("no preset {name}: see `agentplaza models presets`")
                        })?;
                    let mut body = json!({
                        "id": id.clone().unwrap_or_else(|| name.clone()),
                        "kind": ctx::s(&found, "kind"),
                        "base_url": found.get("base_url").cloned().unwrap_or(Value::Null),
                        "auth": found.get("auth").cloned().unwrap_or_else(|| json!("none")),
                        "models": found.get("models").cloned().unwrap_or_else(|| json!([])),
                    });
                    if let Some(options) = found.get("options").filter(|o| !o.is_null()) {
                        body["options"] = options.clone();
                    }
                    body
                }
                None => json!({
                    "id": id.clone().ok_or("give the provider an id with --id")?,
                    "kind": kind.clone().ok_or("give the provider a kind with --kind, or a preset")?,
                    "base_url": Value::Null,
                    "auth": "none",
                    "models": [],
                }),
            };
            if let Some(url) = base_url {
                body["base_url"] = json!(url);
            }
            if let Some(list) = models {
                body["models"] = json!(split_list(&list));
            }
            if let Some(auth) = auth_from_flags(env, secret, command)? {
                body["auth"] = auth;
            }
            let pid = body["id"].as_str().unwrap_or_default().to_string();
            ctx.client.post("/v1/providers", &body)?;
            let stored = key.is_some();
            if let Some(key) = key {
                ctx.client.put(
                    &format!("/v1/providers/{}/secret", ctx::seg(&pid)),
                    &json!({ "value": key }),
                )?;
            }
            ctx.out(&json!({ "id": pid, "key": stored }), |v| {
                println!(
                    "added provider {}{}",
                    ctx::s(v, "id"),
                    if stored { ", key stored" } else { "" }
                );
            });
            Ok(())
        }
        Cmd::Edit {
            id,
            base_url,
            env,
            secret,
            command,
            models,
        } => {
            let list = ctx.client.get("/v1/providers")?;
            let current = items(&list)
                .find(|p| ctx::s(p, "id") == id.as_str())
                .cloned()
                .ok_or_else(|| format!("no provider {id}: see `agentplaza models providers`"))?;
            let mut body = json!({
                "id": id,
                "kind": ctx::s(&current, "kind"),
                "base_url": current.get("base_url").cloned().unwrap_or(Value::Null),
                "auth": current.get("auth").cloned().unwrap_or_else(|| json!("none")),
                "models": current.get("models").cloned().unwrap_or_else(|| json!([])),
                "extra_headers": current.get("extra_headers").cloned().unwrap_or_else(|| json!({})),
            });
            if let Some(options) = current.get("options").filter(|o| !o.is_null()) {
                body["options"] = options.clone();
            }
            if let Some(url) = base_url {
                body["base_url"] = json!(url);
            }
            if let Some(list) = models {
                body["models"] = json!(split_list(&list));
            }
            if let Some(auth) = auth_from_flags(env, secret, command)? {
                body["auth"] = auth;
            }
            let saved = ctx
                .client
                .put(&format!("/v1/providers/{}", ctx::seg(&id)), &body)?;
            ctx.out(&saved, |_| println!("saved provider {id}"));
            Ok(())
        }
        Cmd::Remove { id, yes } => {
            if !confirm(yes, &format!("Remove provider {id}."))? {
                return Err("cancelled".into());
            }
            let gone = ctx
                .client
                .delete(&format!("/v1/providers/{}", ctx::seg(&id)))?;
            ctx.out(&gone, |_| println!("removed provider {id}"));
            Ok(())
        }
        Cmd::Key { id, value } => {
            expect_stdin_marker(&value)?;
            let key = read_secret_from_stdin()?;
            ctx.client.put(
                &format!("/v1/providers/{}/secret", ctx::seg(&id)),
                &json!({ "value": key }),
            )?;
            ctx.out(&json!({ "id": id, "key": "stored" }), |v| {
                println!("key stored for provider {}", ctx::s(v, "id"))
            });
            Ok(())
        }
        Cmd::Test { id, model } => {
            let body = model.map_or_else(|| json!({}), |m| json!({ "model": m }));
            let report = ctx
                .client
                .post(&format!("/v1/providers/{}/test", ctx::seg(&id)), &body)?;
            let ok = report.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let error = ctx::s(&report, "error").to_string();
            ctx.out(&report, |r| {
                if ok {
                    println!(
                        "ok  {}  {} ms  {}",
                        ctx::s(r, "model"),
                        ctx::text(r, "latency_ms"),
                        ctx::s(r, "reply")
                    );
                } else {
                    println!("failed  {}  {}", ctx::s(r, "model"), ctx::s(r, "error"));
                }
            });
            if ok {
                Ok(())
            } else if error.is_empty() {
                Err(format!("the test of {id} failed"))
            } else {
                Err(error)
            }
        }
        Cmd::List { group } => {
            let want = group_filter(group)?;
            let view = ctx.client.get("/v1/models")?;
            let models = view.get("models").cloned().unwrap_or_else(|| json!([]));
            let shown = json!({
                "models": filter(&models, want.as_deref(), model_group),
                "errors": view.get("errors").cloned().unwrap_or_else(|| json!([])),
            });
            ctx.out(&shown, |v| {
                let rows: Vec<Vec<String>> = items(&v["models"])
                    .map(|m| {
                        vec![
                            ctx::s(m, "provider").to_string(),
                            ctx::s(m, "id").to_string(),
                            ctx::text(m, "name"),
                            model_group(m).to_string(),
                            trains_marker(m).to_string(),
                        ]
                    })
                    .collect();
                ctx::table(
                    &["provider", "id", "name", "group", "trains on data"],
                    &rows,
                );
                for e in items(&v["errors"]) {
                    println!(
                        "could not list {}: {}",
                        ctx::s(e, "provider"),
                        ctx::s(e, "error")
                    );
                }
            });
            Ok(())
        }
        Cmd::Policy { action } => match action {
            PolicyCmd::Show => {
                let settings = ctx.client.get("/v1/settings")?;
                ctx.out(&settings, print_policy);
                Ok(())
            }
            PolicyCmd::Set {
                orchestrator,
                worker,
                effort,
                max_turns,
                max_tokens,
                fetch_block,
                fetch_unblock,
                default_model,
                orchestrator_model,
                role_models,
                prices,
                allow_training_models,
                idle_sleep_min,
                sleep_never_roles,
                vm_auto_checkpoints,
            } => {
                let mut settings = ctx.client.get("/v1/settings")?;
                let root = settings
                    .as_object_mut()
                    .ok_or("the hub's settings are not an object")?;
                if let Some(list) = orchestrator {
                    nested(root, "model_policy")?
                        .insert("orchestrator".into(), json!(entries(&list)?));
                }
                if let Some(list) = worker {
                    nested(root, "model_policy")?.insert("worker".into(), json!(entries(&list)?));
                }
                if let Some(level) = effort {
                    set_effort(root, &level)?;
                }
                if let Some(n) = max_turns {
                    root.insert("max_turns".into(), json!(n));
                }
                if let Some(minutes) = idle_sleep_min {
                    root.insert("idle_sleep_min".into(), json!(minutes));
                }
                if let Some(roles) = sleep_never_roles {
                    root.insert("sleep_never_roles".into(), json!(split_list(&roles)));
                }
                if let Some(on) = vm_auto_checkpoints {
                    root.insert("vm_auto_checkpoints".into(), json!(on));
                }
                if let Some(n) = max_tokens {
                    root.insert("max_tokens".into(), json!(n));
                }
                if !fetch_block.is_empty() || !fetch_unblock.is_empty() {
                    let add = fetch_block
                        .iter()
                        .map(|h| bare_host(h))
                        .collect::<Result<Vec<_>, _>>()?;
                    let remove = fetch_unblock
                        .iter()
                        .map(|h| bare_host(h))
                        .collect::<Result<Vec<_>, _>>()?;
                    edit_blocklist(root, &add, &remove);
                }
                if let Some(model) = default_model {
                    root.insert("default_model".into(), json!(checked_entry(&model)?));
                }
                if let Some(model) = orchestrator_model {
                    root.insert("orchestrator_model".into(), json!(checked_entry(&model)?));
                }
                for pair in role_models {
                    let (role, model) = pair
                        .split_once('=')
                        .ok_or_else(|| format!("--role-model takes ROLE=MODEL, not {pair}"))?;
                    let roles = nested(root, "role_models")?;
                    if model.is_empty() {
                        roles.remove(role);
                    } else {
                        roles.insert(role.to_string(), json!(checked_entry(model)?));
                    }
                }
                for pair in prices {
                    let (model, rate) = pair
                        .split_once('=')
                        .ok_or_else(|| format!("--price takes MODEL=IN:OUT, not {pair}"))?;
                    let (input, output) = rate
                        .split_once(':')
                        .ok_or_else(|| format!("--price takes MODEL=IN:OUT, not {pair}"))?;
                    let input: f64 = input
                        .parse()
                        .map_err(|_| format!("the input price in {pair} is not a number"))?;
                    let output: f64 = output
                        .parse()
                        .map_err(|_| format!("the output price in {pair} is not a number"))?;
                    nested(root, "prices")?.insert(
                        model.to_string(),
                        json!({ "input": input, "output": output }),
                    );
                }
                if let Some(allow) = allow_training_models {
                    root.insert("allow_training_models".into(), json!(allow));
                }
                let saved = ctx.client.put("/v1/settings", &settings)?;
                ctx.out(&saved, print_policy);
                Ok(())
            }
        },
        Cmd::Price { model } => {
            let v = ctx.client.get(&price_path(&model))?;
            ctx.out(&v, print_price);
            Ok(())
        }
        Cmd::Prices {
            action: Some(PricesCmd::Refresh),
        } => {
            let v = ctx.client.call("POST", "/v1/prices/refresh", None)?;
            ctx.out(&v, print_refresh);
            Ok(())
        }
        Cmd::Prices { action: None } => {
            let v = ctx.client.get("/v1/prices")?;
            ctx.out(&v, print_prices);
            Ok(())
        }
        Cmd::Decisions {
            action: DecisionsCmd::Current { provider, model },
        } => match (provider, model) {
            (None, None) => {
                let current = ctx.client.get("/v1/decisions/default")?;
                ctx.out(&current, print_decision);
                Ok(())
            }
            (Some(provider), Some(model)) => {
                let saved = ctx.client.put(
                    "/v1/decisions/default",
                    &json!({ "provider": provider, "model": model }),
                )?;
                ctx.out(&saved, print_decision);
                Ok(())
            }
            _ => Err("give both a provider and a model, or neither".into()),
        },
    }
}

// ---- Helpers shared with the other console areas ----------------------------------------------

/// The items of a JSON array (none when the answer is not an array).
pub(crate) fn items(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

/// The string items of an optional array, joined with commas ("" when there are none).
pub(crate) fn joined(v: Option<&Value>) -> String {
    let parts: Vec<&str> = v
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    parts.join(", ")
}

/// `(none)` for an empty answer, else the text itself.
pub(crate) fn none_if_empty(text: String) -> String {
    if text.is_empty() {
        "(none)".into()
    } else {
        text
    }
}

/// Splits a comma-separated flag, dropping blanks.
pub(crate) fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The key on stdin, trimmed. Never an argument, never echoed.
pub(crate) fn read_secret_from_stdin() -> Result<String, String> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read the key from stdin: {e}"))?;
    let key = text.trim().to_string();
    if key.is_empty() {
        Err("stdin held no key".into())
    } else {
        Ok(key)
    }
}

/// A key is only ever read from stdin: the value flag must be `-`.
pub(crate) fn expect_stdin_marker(value: &str) -> Result<(), String> {
    if value == "-" {
        Ok(())
    } else {
        Err("a key is never an argument: pass --value - and send the key on stdin".into())
    }
}

// ---- Providers ----------------------------------------------------------------------------------

/// The providers or models in `group` (all when None), as an array.
fn filter(list: &Value, group: Option<&str>, group_of: fn(&Value) -> &'static str) -> Value {
    Value::Array(
        items(list)
            .filter(|p| group.is_none_or(|g| group_of(p) == g))
            .cloned()
            .collect(),
    )
}

/// `--group`: chat or decisions (or none).
fn group_filter(group: Option<String>) -> Result<Option<String>, String> {
    let ok = matches!(group.as_deref(), None | Some("chat") | Some("decisions"));
    if ok {
        Ok(group)
    } else {
        Err(format!(
            "--group is chat or decisions, not {}",
            group.unwrap_or_default()
        ))
    }
}

/// A provider's group: its own `group` when the hub says, else by its kind.
fn provider_group(p: &Value) -> &'static str {
    match p.get("group").and_then(Value::as_str) {
        Some(group) => {
            if group == "decisions" {
                "decisions"
            } else {
                "chat"
            }
        }
        None if DECISION_KINDS.contains(&ctx::s(p, "kind")) => "decisions",
        None => "chat",
    }
}

/// A model's group: decisions only when marked so; every other model is chat.
fn model_group(m: &Value) -> &'static str {
    if m.get("group").and_then(Value::as_str) == Some("decisions") {
        "decisions"
    } else {
        "chat"
    }
}

/// `yes` when the hub marks the model as one whose vendor trains on the data sent to it; else empty.
fn trains_marker(m: &Value) -> &'static str {
    if m.get("trains_on_data").and_then(Value::as_bool) == Some(true) {
        "yes"
    } else {
        ""
    }
}

/// The key status word for a provider's credential.
fn key_status(credential: &str) -> &'static str {
    match credential {
        "env" | "secret" => "key set",
        "none" => "no key needed",
        "command" => "command",
        "missing" => "key missing",
        _ => "unknown",
    }
}

/// An auth as text: `none`, or `env NAME`, `secret NAME`, `command …`.
fn auth_label(auth: Option<&Value>) -> String {
    match auth {
        None | Some(Value::Null) => "none".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(map)) => map
            .iter()
            .next()
            .map(|(kind, v)| format!("{kind} {}", v.as_str().unwrap_or("")))
            .unwrap_or_default(),
        Some(other) => other.to_string(),
    }
}

/// The auth the flags name: at most one of `--env`, `--secret`, `--command`. None when no flag is given.
fn auth_from_flags(
    env: Option<String>,
    secret: Option<String>,
    command: Option<String>,
) -> Result<Option<Value>, String> {
    let picked: Vec<Value> = [
        env.map(|name| json!({ "env": name })),
        secret.map(|name| json!({ "secret": name })),
        command.map(|cmd| json!({ "command": cmd })),
    ]
    .into_iter()
    .flatten()
    .collect();
    match picked.len() {
        0 => Ok(None),
        1 => Ok(picked.into_iter().next()),
        _ => Err("pick one of --env, --secret, --command".into()),
    }
}

// ---- Policy and decisions -----------------------------------------------------------------------

/// `provider/model`, optionally `@effort` (the policy's entry form). Only the base is checked.
fn checked_entry(entry: &str) -> Result<String, String> {
    let base = entry.split('@').next().unwrap_or_default();
    let ok = match base.split_once('/') {
        Some((provider, model)) => {
            !provider.is_empty() && !model.is_empty() && !entry.chars().any(char::is_whitespace)
        }
        None => false,
    };
    if ok {
        Ok(entry.to_string())
    } else {
        Err(format!(
            "{entry}: use provider/model, with an optional @effort"
        ))
    }
}

/// A comma-separated list of policy entries, each checked.
fn entries(list: &str) -> Result<Vec<String>, String> {
    split_list(list).iter().map(|e| checked_entry(e)).collect()
}

/// A fetch-blocklist host as the hub takes it: `example.com`, or `*.example.com` for subdomains only.
fn bare_host(text: &str) -> Result<String, String> {
    let host = text.trim();
    let bare = !host.is_empty()
        && !host.contains("://")
        && !host.contains(['/', ':'])
        && !host.chars().any(char::is_whitespace)
        && !host.trim_start_matches("*.").contains('*');
    if bare {
        Ok(host.to_string())
    } else {
        Err(format!(
            "{text}: a host is example.com, or *.example.com for its subdomains only"
        ))
    }
}

/// Edits the hub's fetch blocklist in `root`: the hosts in `add` join it, the hosts in `remove` leave it.
fn edit_blocklist(root: &mut serde_json::Map<String, Value>, add: &[String], remove: &[String]) {
    let mut list: Vec<String> = root
        .get("fetch_block")
        .and_then(Value::as_array)
        .map(|hosts| {
            hosts
                .iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for host in add {
        if !list.contains(host) {
            list.push(host.clone());
        }
    }
    list.retain(|host| !remove.contains(host));
    root.insert("fetch_block".into(), json!(list));
}

/// Sets `effort` in `root` from a `--effort` value; `default` clears it (null, the provider's own default).
fn set_effort(root: &mut serde_json::Map<String, Value>, level: &str) -> Result<(), String> {
    if !EFFORTS.contains(&level) {
        return Err(format!("effort is one of {}", EFFORTS.join(", ")));
    }
    root.insert(
        "effort".into(),
        if level == "default" {
            Value::Null
        } else {
            json!(level)
        },
    );
    Ok(())
}

/// The object under `key` in `parent`, made when it is missing.
fn nested<'a>(
    parent: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, Value>, String> {
    parent
        .entry(key.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("the hub's settings have a non-object {key}"))
}

/// The policy, defaults, limits and prices, as the policy screen shows them.
fn print_policy(s: &Value) {
    let policy = s.get("model_policy");
    ctx::fields(&[
        (
            "orchestrator models",
            none_if_empty(joined(policy.and_then(|p| p.get("orchestrator")))),
        ),
        (
            "worker models",
            none_if_empty(joined(policy.and_then(|p| p.get("worker")))),
        ),
        (
            "orchestrator",
            none_if_empty(ctx::text(s, "orchestrator_model")),
        ),
        (
            "worker default",
            none_if_empty(ctx::text(s, "default_model")),
        ),
        ("effort", none_if_empty(ctx::text(s, "effort"))),
        ("max turns", ctx::text(s, "max_turns")),
        ("max tokens", ctx::text(s, "max_tokens")),
        ("fetch block", none_if_empty(joined(s.get("fetch_block")))),
        (
            "training models",
            if s.get("allow_training_models").and_then(Value::as_bool) == Some(true) {
                "allowed"
            } else {
                "not allowed"
            }
            .to_string(),
        ),
    ]);
    for (role, model) in s
        .get("role_models")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        println!("role {role}: {}", model.as_str().unwrap_or(""));
    }
    for (model, rate) in s
        .get("prices")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        println!(
            "price {model}: in {} out {} per 1M",
            ctx::text(rate, "input"),
            ctx::text(rate, "output")
        );
    }
}

/// The path of one model's price: the model id is one path segment, so its `/` is encoded.
fn price_path(model: &str) -> String {
    format!("/v1/prices/{}", ctx::seg(model))
}

/// A rate, or a dash when the source does not give one.
fn rate(price: &Value, key: &str) -> String {
    match ctx::text(price, key) {
        text if text.is_empty() => "—".to_string(),
        text => text,
    }
}

/// One model's price, as `usage prices` shows it: a rate the source does not give reads as a dash.
fn print_price(v: &Value) {
    let price = v.get("price").unwrap_or(&Value::Null);
    ctx::fields(&[
        ("model", ctx::text(v, "model")),
        ("input", rate(price, "input")),
        ("output", rate(price, "output")),
        ("cache read", rate(price, "cache_read")),
        ("cache write", rate(price, "cache_write")),
        ("source", ctx::s(price, "source").to_string()),
    ]);
    println!();
    println!("USD per million tokens");
}

/// One row per price source a refresh fetched: its name, how many models it gave, whether it worked, and why not.
fn source_rows(v: &Value) -> Vec<Vec<String>> {
    items(v.get("sources").unwrap_or(&Value::Null))
        .map(|s| {
            vec![
                ctx::s(s, "name").to_string(),
                ctx::text(s, "models"),
                ctx::text(s, "ok"),
                rate(s, "error"),
            ]
        })
        .collect()
}

/// One row per price: the model, its input and output rates, and the source they came from.
fn price_rows(v: &Value) -> Vec<Vec<String>> {
    items(v.get("models").unwrap_or(&Value::Null))
        .map(|m| {
            let price = m.get("price").unwrap_or(&Value::Null);
            vec![
                ctx::text(m, "model"),
                rate(price, "input"),
                rate(price, "output"),
                ctx::s(price, "source").to_string(),
            ]
        })
        .collect()
}

/// What a refresh fetched: when, and each source's outcome.
fn print_refresh(v: &Value) {
    println!(
        "refreshed the price sources, fetched {}",
        ctx::when(v.get("fetched_at_ms").unwrap_or(&Value::Null))
    );
    println!();
    ctx::table(&["source", "models", "ok", "error"], &source_rows(v));
}

/// Every price the plaza knows, as `usage prices` lists them.
fn print_prices(v: &Value) {
    println!("USD per million tokens");
    ctx::table(&["model", "input", "output", "source"], &price_rows(v));
}

/// The decision model, as the decisions screen names it.
fn print_decision(d: &Value) {
    ctx::fields(&[
        ("provider", none_if_empty(ctx::text(d, "provider"))),
        ("model", none_if_empty(ctx::text(d, "model"))),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Just the `models` command, so the policy subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `models policy set ARGS`, parsed.
    fn policy_set(args: &[&str]) -> Result<PolicyCmd, clap::Error> {
        let mut argv = vec!["models", "policy", "set"];
        argv.extend_from_slice(args);
        match Probe::try_parse_from(argv)?.cmd {
            Cmd::Policy { action } => Ok(action),
            other => panic!("not a policy command: {other:?}"),
        }
    }

    #[test]
    fn policy_set_parses_its_flags() {
        // The fetch flags repeat, and the old --fetch-allow flag is gone.
        {
            let PolicyCmd::Set {
                fetch_block,
                fetch_unblock,
                ..
            } = policy_set(&[
                "--fetch-block",
                "evil.example",
                "--fetch-block",
                "*.ads.net",
                "--fetch-unblock",
                "docs.example",
            ])
            .expect("repeatable flags parse")
            else {
                panic!("expected set");
            };
            assert_eq!(
                fetch_block,
                vec!["evil.example".to_string(), "*.ads.net".to_string()]
            );
            assert_eq!(fetch_unblock, vec!["docs.example".to_string()]);
            assert!(
                policy_set(&["--fetch-allow", "example.com"]).is_err(),
                "the flag the hub never read is gone"
            );
        }
        // --allow-training-models takes true or false and refuses anything else.
        {
            let PolicyCmd::Set {
                allow_training_models,
                ..
            } = policy_set(&["--allow-training-models", "true"]).expect("true parses")
            else {
                panic!("expected set");
            };
            assert_eq!(allow_training_models, Some(true));
            let PolicyCmd::Set {
                allow_training_models,
                ..
            } = policy_set(&["--allow-training-models", "false"]).expect("false parses")
            else {
                panic!("expected set");
            };
            assert_eq!(allow_training_models, Some(false));
            assert!(policy_set(&["--allow-training-models", "maybe"]).is_err());
        }
    }

    #[test]
    fn bare_host_takes_example_com_and_wildcards_and_refuses_urls_and_paths() {
        assert_eq!(bare_host("example.com").unwrap(), "example.com");
        assert_eq!(bare_host(" *.example.net ").unwrap(), "*.example.net");
        for bad in [
            "https://example.com",
            "example.com/path",
            "host:8080",
            "has space.com",
            "",
            "*",
            "a.*.com",
        ] {
            assert!(bare_host(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn edit_blocklist_adds_once_and_removes_in_place() {
        let mut root = serde_json::Map::new();
        root.insert("fetch_block".into(), json!(["old.example"]));
        edit_blocklist(&mut root, &["a.example".into(), "old.example".into()], &[]);
        assert_eq!(root["fetch_block"], json!(["old.example", "a.example"]));
        edit_blocklist(&mut root, &["*.b.example".into()], &["old.example".into()]);
        assert_eq!(root["fetch_block"], json!(["a.example", "*.b.example"]));
    }

    #[test]
    fn effort_takes_the_hubs_levels_and_default_clears_it() {
        let mut root = serde_json::Map::new();
        for level in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            set_effort(&mut root, level).unwrap();
            assert_eq!(root["effort"], json!(level));
        }
        set_effort(&mut root, "default").unwrap();
        assert_eq!(
            root["effort"],
            Value::Null,
            "default clears the setting to unset"
        );
        let err = set_effort(&mut root, "extreme").unwrap_err();
        assert_eq!(
            err,
            "effort is one of none, minimal, low, medium, high, xhigh, max, default"
        );
    }

    #[test]
    fn a_price_path_is_one_encoded_segment() {
        assert_eq!(
            price_path("anthropic/claude-x"),
            "/v1/prices/anthropic%2Fclaude-x"
        );
        assert_eq!(price_path("glm-5.3"), "/v1/prices/glm-5.3");
    }

    #[test]
    fn price_takes_a_model_and_prices_takes_refresh_or_nothing() {
        assert!(
            matches!(Probe::try_parse_from(["models", "price", "anthropic/claude-x"]).map(|p| p.cmd), Ok(Cmd::Price { model }) if model == "anthropic/claude-x")
        );
        assert!(matches!(
            Probe::try_parse_from(["models", "prices", "refresh"]).map(|p| p.cmd),
            Ok(Cmd::Prices {
                action: Some(PricesCmd::Refresh)
            })
        ));
        assert!(matches!(
            Probe::try_parse_from(["models", "prices"]).map(|p| p.cmd),
            Ok(Cmd::Prices { action: None })
        ));
    }
}
