//! `agentplaza models`: Model providers, keys, policy, decision models.

use std::io::Read;

use clap::Subcommand;
use serde_json::{Map, Value, json};

use crate::ctx::{self, Ctx, confirm};

/// The provider kinds whose models are decision models (as the console decides it).
const DECISION_KINDS: [&str; 3] = ["openai-decisions", "systemone", "azure-decisions"];
/// The effort levels the hub's settings accept, and `default`, which clears the setting (the provider's own default).
const EFFORTS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "default",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The configured providers and the coding engines: label, enabled, health, key, endpoint, models.
    Providers {
        /// Only the `chat` or the `decisions` API providers (engines have no group, so they are left out).
        #[arg(long)]
        group: Option<String>,
        /// Only the `api` providers or the `engine` instances.
        #[arg(long, value_name = "api|engine")]
        family: Option<String>,
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
        /// The name shown for the provider (default: the kind's name).
        #[arg(long)]
        label: Option<String>,
        /// The accent colour, #rrggbb (`none` clears it).
        #[arg(long)]
        accent: Option<String>,
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
    /// Turn a provider or a coding engine on.
    Enable {
        /// The provider or engine id.
        id: String,
    },
    /// Turn a provider or a coding engine off.
    Disable {
        /// The provider or engine id.
        id: String,
    },
    /// Check the health of one provider or engine, or of all of them.
    Check {
        /// The provider or engine id; none checks all.
        id: Option<String>,
    },
    /// Update the coding engines to the versions the catalogue pins.
    Update {
        /// List the changes without installing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// The coding engines: add, change, remove.
    Engines {
        #[command(subcommand)]
        action: EnginesCmd,
    },
    /// The ACP registry of coding agents.
    Registry {
        #[command(subcommand)]
        action: RegistryCmd,
    },
    /// Favourite models: they lead the model pickers.
    Favourite {
        #[command(subcommand)]
        action: FavouriteCmd,
    },
    /// The CLIProxyAPI hubs whose pooled account quotas show next to the providers.
    UsageProviders {
        #[command(subcommand)]
        action: UsageCmd,
    },
    /// The providers' settings: the background health check interval.
    Settings {
        #[command(subcommand)]
        action: SettingsCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum PricesCmd {
    /// Fetches the price sources again now, then prints what each one returned.
    Refresh,
}

/// The flags an engine takes (`engines add` and `engines set`). Variables are plain values: keys never go here.
#[derive(clap::Args, Debug, Default)]
pub struct EngineFlags {
    /// The name shown for the engine.
    #[arg(long)]
    pub label: Option<String>,
    /// A local program to run (one program; its arguments go in --arg).
    #[arg(long)]
    pub command: Option<String>,
    /// One argument for the program, in order. Repeatable.
    #[arg(long = "arg", value_name = "ARG", allow_hyphen_values = true)]
    pub args: Vec<String>,
    /// A plain variable for the engine, NAME=VALUE. Repeatable.
    #[arg(long = "env", value_name = "NAME=VALUE")]
    pub env: Vec<String>,
    /// The model the engine runs through, provider/model (empty clears it: the plaza's default).
    #[arg(long)]
    pub route: Option<String>,
    /// The npm version: a version, `latest`, or `pinned` (the catalogue's).
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum EnginesCmd {
    /// Add an engine: from the ACP registry (`--registry`), or as a local program (`--command`).
    #[command(disable_version_flag = true)]
    Add {
        /// The engine's id: letters, digits, `_` and `-`.
        id: String,
        /// The registry agent to run, e.g. claude-code (see `models registry search`).
        #[arg(long)]
        registry: Option<String>,
        #[command(flatten)]
        flags: EngineFlags,
    },
    /// Change an engine. Flags not given keep what it has; a flag given replaces its list (args, variables).
    #[command(disable_version_flag = true)]
    Set {
        /// The engine's id.
        id: String,
        #[command(flatten)]
        flags: EngineFlags,
    },
    /// Remove an engine you added (the built-in ones are only turned off). Asks first unless --yes.
    Remove {
        /// The engine's id.
        id: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RegistryCmd {
    /// The registry's agents whose id, name or description contains the query (all when empty).
    Search {
        /// The text to find.
        #[arg(default_value = "")]
        query: String,
        /// Fetch the registry again first instead of the hub's copy.
        #[arg(long)]
        refresh: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum FavouriteCmd {
    /// Favourite a model: provider/model.
    Add {
        /// The model, provider/model.
        model: String,
    },
    /// Take a model off the favourites.
    Remove {
        /// The model, provider/model.
        model: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum UsageCmd {
    /// Add a CLIProxyAPI hub. Its management key is read from stdin, never an argument.
    Add {
        /// The hub's URL.
        #[arg(long)]
        url: String,
        /// The name shown for it (default: the hub's host name).
        #[arg(long)]
        label: Option<String>,
        /// Read the management key from stdin.
        #[arg(long)]
        key_stdin: bool,
    },
    /// The usage hubs and their last reading.
    List,
    /// One hub's usage now: totals, models, pooled accounts.
    Usage {
        /// The usage provider's id.
        id: String,
    },
    /// Remove a usage hub. Asks first unless --yes.
    Remove {
        /// The usage provider's id.
        id: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SettingsCmd {
    /// How often the background check refreshes the providers, engines and models, in seconds (0 turns it off).
    HealthInterval {
        /// The interval in seconds, 0 to 86400. None shows it.
        seconds: Option<u64>,
    },
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
        /// One model's price in USD per 1M tokens, MODEL=INPUT:OUTPUT. Repeatable.
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
        Cmd::Providers { group, family } => {
            let want = group_filter(group)?;
            let family = family_filter(family.as_deref())?;
            let mut rows: Vec<Value> = Vec::new();
            if family != Some(Family::Engine) {
                let list = ctx.client.get("/v1/providers")?;
                rows.extend(items(&filter(&list, want.as_deref(), provider_group)).cloned());
            }
            if family != Some(Family::Api) && want.is_none() {
                let engines = ctx.client.get("/v1/engines")?;
                rows.extend(items(&engines).cloned());
            }
            let shown = Value::Array(rows);
            ctx.out(&shown, |v| {
                let rows: Vec<Vec<String>> = items(v).map(instance_row).collect();
                ctx::table(
                    &[
                        "family", "id", "kind", "label", "enabled", "health", "key", "endpoint",
                        "models",
                    ],
                    &rows,
                );
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
            label,
            accent,
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
                    preset_body(&id.clone().unwrap_or_else(|| name.clone()), &found)
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
            if let Some(name) = label {
                body["label"] = json!(name);
            }
            if let Some(hex) = accent {
                body["accent"] = accent_value(&hex)?;
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
            let mut body = api_config(&current);
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
        Cmd::Enable { id } => set_enabled(ctx, &id, true),
        Cmd::Disable { id } => set_enabled(ctx, &id, false),
        Cmd::Check { id } => check(ctx, id),
        Cmd::Update { dry_run } => {
            let report = ctx
                .client
                .post("/v1/engines/update", &json!({ "dry_run": dry_run }))?;
            ctx.out(&report, print_update);
            Ok(())
        }
        Cmd::Engines { action } => engines(ctx, action),
        Cmd::Registry {
            action: RegistryCmd::Search { query, refresh },
        } => search_registry(ctx, &query, refresh),
        Cmd::Favourite { action } => favourite(ctx, action),
        Cmd::UsageProviders { action } => usage_providers(ctx, action),
        Cmd::Settings {
            action: SettingsCmd::HealthInterval { seconds },
        } => health_interval(ctx, seconds),
    }
}

// ---- Providers, engines, favourites, usage hubs and settings ----------------------------------

/// Which kind of instance an id names: an API provider, or a coding engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    Api,
    Engine,
}

/// The largest health check interval the hub takes, in seconds (a day).
const MAX_HEALTH_INTERVAL: u64 = 86_400;

/// `--family`: api, engine, or none for both.
fn family_filter(text: Option<&str>) -> Result<Option<Family>, String> {
    match text {
        None => Ok(None),
        Some("api") => Ok(Some(Family::Api)),
        Some("engine") => Ok(Some(Family::Engine)),
        Some(other) => Err(format!("--family is api or engine, not {other}")),
    }
}

/// The provider or engine with `id`, and which family it is (providers are looked at first).
fn find_instance(ctx: &Ctx, id: &str) -> Result<(Family, Value), String> {
    let providers = ctx.client.get("/v1/providers")?;
    if let Some(found) = items(&providers).find(|p| ctx::s(p, "id") == id) {
        return Ok((Family::Api, found.clone()));
    }
    let engines = ctx.client.get("/v1/engines")?;
    items(&engines)
        .find(|e| ctx::s(e, "id") == id)
        .map(|e| (Family::Engine, e.clone()))
        .ok_or_else(|| format!("no provider or engine {id}: see `agentplaza models providers`"))
}

/// The whole config of an API provider as its `PUT` takes it, from what the hub lists. Redacted header values are
/// restored by the hub, so they go back as they are.
pub(crate) fn api_config(current: &Value) -> Value {
    let mut body = json!({
        "id": ctx::s(current, "id"),
        "kind": ctx::s(current, "kind"),
        "label": current.get("label").cloned().unwrap_or(Value::Null),
        "accent": current.get("accent").cloned().unwrap_or(Value::Null),
        "enabled": current.get("enabled").cloned().unwrap_or(json!(true)),
        "base_url": current.get("base_url").cloned().unwrap_or(Value::Null),
        "auth": current.get("auth").cloned().unwrap_or_else(|| json!("none")),
        "models": current.get("models").cloned().unwrap_or_else(|| json!([])),
        "extra_headers": current.get("extra_headers").cloned().unwrap_or_else(|| json!({})),
    });
    for key in ["options", "extra_body"] {
        if let Some(value) = current.get(key).filter(|v| !v.is_null()) {
            body[key] = value.clone();
        }
    }
    body
}

/// The body that adds an API provider from a preset (one of `/v1/provider-kinds`), under `id`.
pub(crate) fn preset_body(id: &str, preset: &Value) -> Value {
    let mut body = json!({
        "id": id,
        "kind": ctx::s(preset, "kind"),
        "base_url": preset.get("base_url").cloned().unwrap_or(Value::Null),
        "auth": preset.get("auth").cloned().unwrap_or_else(|| json!("none")),
        "models": preset.get("models").cloned().unwrap_or_else(|| json!([])),
    });
    if let Some(options) = preset.get("options").filter(|o| !o.is_null()) {
        body["options"] = options.clone();
    }
    body
}

/// `#rrggbb` as the accent the hub keeps; `none` clears it.
fn accent_value(text: &str) -> Result<Value, String> {
    if text == "none" {
        return Ok(Value::Null);
    }
    let hex = text.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(json!(format!("#{}", hex.to_ascii_lowercase())))
    } else {
        Err(format!("{text}: the accent is #rrggbb, or none"))
    }
}

/// An id a provider or engine takes: letters, digits, `_` and `-`.
pub(crate) fn checked_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!("{id}: an id is letters, digits, _ and -"))
    }
}

/// An engine's version as the hub takes it: `pinned`, `latest`, or a dotted version such as `1.2.3`.
pub(crate) fn version_ok(text: &str) -> Result<String, String> {
    let numeric = !text.is_empty()
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if text == "pinned" || text == "latest" || numeric {
        Ok(text.to_string())
    } else {
        Err(format!("{text}: a version is pinned, latest, or x.y.z"))
    }
}

/// The body that saves an engine's runtime and variables (the whole set, as the console's Runtime section sends it):
/// an empty home or route clears it; the arguments and variables replace the lists the hub keeps.
pub(crate) fn runtime_body(
    version: &str,
    home: &str,
    route: &str,
    args: &[String],
    env: &[String],
) -> Result<Value, String> {
    let route = if route.is_empty() {
        Value::Null
    } else {
        json!(checked_entry(route)?)
    };
    Ok(json!({
        "version": version_ok(version)?,
        "home": if home.is_empty() { Value::Null } else { json!(home) },
        "route": route,
        "args": args,
        "env": Value::Object(env_map(env)?),
    }))
}

/// The body that adds a CLIProxyAPI usage hub: its management key goes with it, and the hub keeps it.
pub(crate) fn usage_body(url: &str, key: &str, label: Option<&str>) -> Value {
    let mut body = json!({ "kind": "cliproxyapi", "url": url, "key": key });
    if let Some(label) = label {
        body["label"] = json!(label);
    }
    body
}

/// `NAME=VALUE` pairs as the plain variables an engine takes.
pub(crate) fn env_map(pairs: &[String]) -> Result<Map<String, Value>, String> {
    let mut env = Map::new();
    for pair in pairs {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("--env takes NAME=VALUE, not {pair}"))?;
        let name_ok = !name.is_empty()
            && name
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()));
        if !name_ok {
            return Err(format!(
                "{name}: a variable name is letters, digits and _, not starting with a digit"
            ));
        }
        env.insert(name.to_string(), json!(value));
    }
    Ok(env)
}

/// The body that adds an engine: from the ACP registry (`registry`), or a local program (`flags.command`).
pub(crate) fn engine_body(
    id: &str,
    registry: Option<&str>,
    flags: &EngineFlags,
) -> Result<Value, String> {
    let engine = match (registry, flags.command.as_deref()) {
        (Some(_), Some(_)) => {
            return Err("--registry and --command are two ways to get an engine: pick one".into());
        }
        (Some(agent), None) => format!("acp:{agent}"),
        (None, Some(_)) => "acp-local".to_string(),
        (None, None) => {
            return Err("give the engine a registry agent with --registry, or a local program with --command".into());
        }
    };
    let mut body = json!({ "id": id, "engine": engine });
    if let Some(label) = &flags.label {
        body["label"] = json!(label);
    }
    if let Some(command) = &flags.command {
        body["command"] = json!(command);
    }
    if !flags.args.is_empty() {
        body["args"] = json!(flags.args);
    }
    if !flags.env.is_empty() {
        body["env"] = Value::Object(env_map(&flags.env)?);
    }
    if let Some(route) = flags.route.as_deref().filter(|r| !r.is_empty()) {
        body["route"] = json!(checked_entry(route)?);
    }
    if let Some(version) = flags.version.as_deref().filter(|v| !v.is_empty()) {
        body["version"] = json!(version_ok(version)?);
    }
    Ok(body)
}

/// The fields `engines set` changes: only the flags given. Refuses an empty change.
pub(crate) fn engine_edit_body(flags: &EngineFlags) -> Result<Value, String> {
    let mut body = Map::new();
    if let Some(label) = &flags.label {
        body.insert("label".into(), json!(label));
    }
    if let Some(command) = &flags.command {
        body.insert("command".into(), json!(command));
    }
    if !flags.args.is_empty() {
        body.insert("args".into(), json!(flags.args));
    }
    if !flags.env.is_empty() {
        body.insert("env".into(), Value::Object(env_map(&flags.env)?));
    }
    if let Some(route) = &flags.route {
        let value = if route.is_empty() {
            Value::Null
        } else {
            json!(checked_entry(route)?)
        };
        body.insert("route".into(), value);
    }
    if let Some(version) = flags.version.as_deref().filter(|v| !v.is_empty()) {
        body.insert("version".into(), json!(version_ok(version)?));
    }
    if body.is_empty() {
        return Err(
            "nothing to change: give a flag (see `agentplaza models engines set --help`)".into(),
        );
    }
    Ok(Value::Object(body))
}

/// Turns a provider or an engine on or off. An API provider is saved whole; an engine takes just the flag.
fn set_enabled(ctx: &Ctx, id: &str, on: bool) -> Result<(), String> {
    let (family, current) = find_instance(ctx, id)?;
    let saved = match family {
        Family::Api => {
            let mut body = api_config(&current);
            body["enabled"] = json!(on);
            ctx.client
                .put(&format!("/v1/providers/{}", ctx::seg(id)), &body)?
        }
        Family::Engine => ctx.client.put(
            &format!("/v1/engines/{}", ctx::seg(id)),
            &json!({ "enabled": on }),
        )?,
    };
    let verb = if on { "enabled" } else { "disabled" };
    ctx.out(&saved, |_| println!("{verb} {id}"));
    Ok(())
}

/// Checks one instance, or all of them. The answer is a map of id to health, as the hub's check answers are.
fn check(ctx: &Ctx, id: Option<String>) -> Result<(), String> {
    let report = match id {
        Some(id) => {
            let (family, _) = find_instance(ctx, &id)?;
            let health = match family {
                Family::Api => {
                    let answer = ctx.client.call(
                        "POST",
                        &format!("/v1/providers/{}/check", ctx::seg(&id)),
                        None,
                    )?;
                    answer.get("health").cloned().unwrap_or(Value::Null)
                }
                Family::Engine => {
                    let map = ctx.client.call("POST", "/v1/engines/check", None)?;
                    map.get(&id).cloned().unwrap_or(Value::Null)
                }
            };
            let mut one = Map::new();
            one.insert(id, health);
            Value::Object(one)
        }
        None => {
            let apis = ctx.client.call("POST", "/v1/providers/check", None)?;
            let engines = ctx.client.call("POST", "/v1/engines/check", None)?;
            let mut all = Map::new();
            for map in [apis, engines] {
                if let Value::Object(entries) = map {
                    all.extend(entries);
                }
            }
            Value::Object(all)
        }
    };
    ctx.out(&report, print_health);
    Ok(())
}

/// One row per checked instance: its id, its status and its message.
fn print_health(map: &Value) {
    let rows: Vec<Vec<String>> = map
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, h)| {
            vec![
                id.clone(),
                ctx::s(h, "status").to_string(),
                ctx::text(h, "message"),
            ]
        })
        .collect();
    ctx::table(&["id", "status", "message"], &rows);
}

/// The health of an instance as one cell: its status, then its message.
pub(crate) fn health_text(h: &Value) -> String {
    let status = ctx::s(h, "status");
    let message = ctx::s(h, "message");
    if message.is_empty() {
        status.to_string()
    } else {
        format!("{status}  {message}")
    }
}

/// One row of the providers list: an API provider, or an engine (its version where it has one).
fn instance_row(i: &Value) -> Vec<String> {
    let engine = ctx::s(i, "family") == "engine";
    let endpoint = if engine {
        let version = ctx::text(i, "version");
        none_if_empty(if version.is_empty() {
            ctx::text(i, "pinned")
        } else {
            version
        })
    } else {
        ctx::text(i, "base_url")
    };
    let models = if engine {
        "-".to_string()
    } else {
        i.get("models")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
            .to_string()
    };
    vec![
        if engine { "engine" } else { "api" }.to_string(),
        ctx::s(i, "id").to_string(),
        if engine {
            ctx::s(i, "engine").to_string()
        } else {
            ctx::s(i, "kind").to_string()
        },
        ctx::s(i, "label").to_string(),
        if i.get("enabled").and_then(Value::as_bool) == Some(true) {
            "yes"
        } else {
            "no"
        }
        .to_string(),
        health_text(i.get("health").unwrap_or(&Value::Null)),
        if engine {
            "-".to_string()
        } else {
            key_status(ctx::s(i, "credential")).to_string()
        },
        endpoint,
        models,
    ]
}

/// The update plan's changes, as the dry run and the update both answer them.
fn print_update(v: &Value) {
    let rows: Vec<Vec<String>> = items(&v["changes"])
        .map(|c| {
            vec![
                ctx::s(c, "id").to_string(),
                ctx::s(c, "from").to_string(),
                ctx::s(c, "to").to_string(),
            ]
        })
        .collect();
    if rows.is_empty() {
        println!("Everything is up to date");
        return;
    }
    let verb = if v.get("dry_run").and_then(Value::as_bool) == Some(true) {
        "would update"
    } else {
        "updated"
    };
    println!("{verb} {} engine(s)", rows.len());
    ctx::table(&["engine", "from", "to"], &rows);
}

fn engines(ctx: &Ctx, action: EnginesCmd) -> Result<(), String> {
    match action {
        EnginesCmd::Add {
            id,
            registry,
            flags,
        } => {
            checked_id(&id)?;
            let body = engine_body(&id, registry.as_deref(), &flags)?;
            let saved = ctx.client.post("/v1/engines", &body)?;
            ctx.out(&saved, |_| println!("added engine {id}"));
        }
        EnginesCmd::Set { id, flags } => {
            let body = engine_edit_body(&flags)?;
            let saved = ctx
                .client
                .put(&format!("/v1/engines/{}", ctx::seg(&id)), &body)?;
            ctx.out(&saved, |_| println!("saved engine {id}"));
        }
        EnginesCmd::Remove { id, yes } => {
            if !confirm(yes, &format!("Remove engine {id}."))? {
                return Err("cancelled".into());
            }
            let gone = ctx
                .client
                .delete(&format!("/v1/engines/{}", ctx::seg(&id)))?;
            ctx.out(&gone, |_| println!("removed engine {id}"));
        }
    }
    Ok(())
}

/// Whether a registry agent matches the search text (its id, name or description, in any case; everything when empty).
fn matches_agent(agent: &Value, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    needle.is_empty()
        || ["id", "name", "description"]
            .iter()
            .any(|key| ctx::s(agent, key).to_lowercase().contains(&needle))
}

fn search_registry(ctx: &Ctx, query: &str, refresh: bool) -> Result<(), String> {
    let path = if refresh {
        "/v1/acp/registry?refresh=1"
    } else {
        "/v1/acp/registry"
    };
    let registry = ctx.client.get(path)?;
    let hits: Vec<Value> = items(&registry["agents"])
        .filter(|agent| matches_agent(agent, query))
        .cloned()
        .collect();
    ctx.out(&Value::Array(hits), |v| {
        let rows: Vec<Vec<String>> = items(v)
            .map(|a| {
                vec![
                    ctx::s(a, "id").to_string(),
                    ctx::s(a, "name").to_string(),
                    ctx::s(a, "version").to_string(),
                    none_if_empty(ctx::text(a, "distribution")),
                    if a.get("runs_in_vm").and_then(Value::as_bool) == Some(true) {
                        "yes"
                    } else {
                        "no"
                    }
                    .to_string(),
                    ctx::s(a, "description").to_string(),
                ]
            })
            .collect();
        ctx::table(
            &[
                "id",
                "name",
                "version",
                "source",
                "runs in a VM",
                "description",
            ],
            &rows,
        );
    });
    Ok(())
}

/// Adds or removes one model in `favourite_models` of the settings in `root`. Adding one twice keeps one.
pub(crate) fn edit_favourites(root: &mut Map<String, Value>, model: &str, on: bool) {
    let mut list: Vec<String> = root
        .get("favourite_models")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if on {
        if !list.iter().any(|m| m == model) {
            list.push(model.to_string());
        }
    } else {
        list.retain(|m| m != model);
    }
    root.insert("favourite_models".into(), json!(list));
}

/// A favourite is `provider/model`: the policy's `@effort` does not belong in it.
fn favourite_key(model: &str) -> Result<String, String> {
    if model.contains('@') {
        return Err(format!(
            "{model}: a favourite is provider/model, with no @effort"
        ));
    }
    checked_entry(model)
}

fn favourite(ctx: &Ctx, action: FavouriteCmd) -> Result<(), String> {
    let (model, on) = match action {
        FavouriteCmd::Add { model } => (model, true),
        FavouriteCmd::Remove { model } => (model, false),
    };
    let model = favourite_key(&model)?;
    let mut settings = ctx.client.get("/v1/settings")?;
    let root = settings
        .as_object_mut()
        .ok_or("the hub's settings are not an object")?;
    edit_favourites(root, &model, on);
    let saved = ctx.client.put("/v1/settings", &settings)?;
    ctx.out(&saved, |s| {
        let shown = items(&s["favourite_models"])
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if shown.is_empty() {
            println!("no favourite models");
        } else {
            for model in shown {
                println!("{model}");
            }
        }
    });
    Ok(())
}

fn usage_providers(ctx: &Ctx, action: UsageCmd) -> Result<(), String> {
    match action {
        UsageCmd::Add {
            url,
            label,
            key_stdin,
        } => {
            if !key_stdin {
                return Err("the hub's management key is read from stdin: pass --key-stdin".into());
            }
            let key = read_secret_from_stdin()?;
            let body = usage_body(&url, &key, label.as_deref());
            let saved = ctx.client.post("/v1/usage-providers", &body)?;
            ctx.out(&saved, |v| {
                println!("added usage provider {}", ctx::s(v, "id"));
            });
        }
        UsageCmd::List => {
            let list = ctx.client.get("/v1/usage-providers")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|u| {
                        let last = u.get("last").unwrap_or(&Value::Null);
                        vec![
                            ctx::s(u, "id").to_string(),
                            ctx::s(u, "label").to_string(),
                            ctx::s(u, "url").to_string(),
                            ctx::s(u, "key").to_string(),
                            none_if_empty(ctx::text(last, "status")),
                            none_if_empty(ctx::text(last, "message")),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "label", "url", "key", "status", "message"], &rows);
            });
        }
        UsageCmd::Usage { id } => {
            let reading = ctx
                .client
                .get(&format!("/v1/usage-providers/{}/usage", ctx::seg(&id)))?;
            ctx.out(&reading, print_usage);
        }
        UsageCmd::Remove { id, yes } => {
            if !confirm(yes, &format!("Remove usage provider {id}."))? {
                return Err("cancelled".into());
            }
            let gone = ctx
                .client
                .delete(&format!("/v1/usage-providers/{}", ctx::seg(&id)))?;
            ctx.out(&gone, |_| println!("removed usage provider {id}"));
        }
    }
    Ok(())
}

/// One hub's reading: its status, its totals, its models and its pooled accounts.
fn print_usage(v: &Value) {
    ctx::fields(&[
        ("status", ctx::text(v, "status")),
        ("message", none_if_empty(ctx::text(v, "message"))),
    ]);
    let totals = v.get("totals").unwrap_or(&Value::Null);
    ctx::fields(&[
        ("requests", ctx::text(totals, "requests")),
        ("success", ctx::text(totals, "success")),
        ("failure", ctx::text(totals, "failure")),
        ("tokens", ctx::text(totals, "tokens")),
    ]);
    let models: Vec<Vec<String>> = items(&v["models"])
        .map(|m| {
            vec![
                ctx::s(m, "model").to_string(),
                ctx::text(m, "requests"),
                ctx::text(m, "tokens"),
            ]
        })
        .collect();
    if !models.is_empty() {
        println!();
        ctx::table(&["model", "requests", "tokens"], &models);
    }
    let accounts: Vec<Vec<String>> = items(&v["accounts"])
        .map(|a| {
            vec![
                ctx::s(a, "name").to_string(),
                ctx::s(a, "provider").to_string(),
                ctx::s(a, "status").to_string(),
                ctx::s(a, "message").to_string(),
            ]
        })
        .collect();
    if !accounts.is_empty() {
        println!();
        ctx::table(&["account", "provider", "status", "message"], &accounts);
    }
}

fn health_interval(ctx: &Ctx, seconds: Option<u64>) -> Result<(), String> {
    let mut settings = ctx.client.get("/v1/settings")?;
    let saved = match seconds {
        None => settings,
        Some(n) => {
            if n > MAX_HEALTH_INTERVAL {
                return Err(format!(
                    "the interval is 0 (off) to {MAX_HEALTH_INTERVAL} seconds, not {n}"
                ));
            }
            settings
                .as_object_mut()
                .ok_or("the hub's settings are not an object")?
                .insert("health_interval_s".into(), json!(n));
            ctx.client.put("/v1/settings", &settings)?
        }
    };
    ctx.out(&saved, |s| {
        println!(
            "health check interval: {} seconds (0 = off)",
            ctx::text(s, "health_interval_s")
        );
    });
    Ok(())
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
pub(crate) fn checked_entry(entry: &str) -> Result<String, String> {
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

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["models"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn providers_lists_both_families_and_takes_a_family_filter() {
        assert!(matches!(
            parse(&["providers"]),
            Ok(Cmd::Providers {
                group: None,
                family: None
            })
        ));
        assert!(matches!(
            parse(&["providers", "--family", "engine"]),
            Ok(Cmd::Providers { family: Some(f), .. }) if f == "engine"
        ));
        assert_eq!(family_filter(Some("api")), Ok(Some(Family::Api)));
        assert_eq!(family_filter(None), Ok(None));
        assert!(family_filter(Some("both")).is_err());
    }

    #[test]
    fn add_takes_a_label_and_an_accent() {
        let Ok(Cmd::Add { label, accent, .. }) =
            parse(&["add", "openai", "--label", "Work", "--accent", "#5CC8FF"])
        else {
            panic!("add parses");
        };
        assert_eq!(label.as_deref(), Some("Work"));
        assert_eq!(accent.as_deref(), Some("#5CC8FF"));
        assert_eq!(accent_value("#5CC8FF").unwrap(), json!("#5cc8ff"));
        assert_eq!(accent_value("none").unwrap(), Value::Null);
        assert!(accent_value("blue").is_err());
        assert!(accent_value("#12345").is_err());
    }

    #[test]
    fn enable_disable_check_and_update_parse() {
        assert!(matches!(parse(&["enable", "openai"]), Ok(Cmd::Enable { id }) if id == "openai"));
        assert!(
            matches!(parse(&["disable", "claude-code"]), Ok(Cmd::Disable { id }) if id == "claude-code")
        );
        assert!(matches!(parse(&["check"]), Ok(Cmd::Check { id: None })));
        assert!(
            matches!(parse(&["check", "openai"]), Ok(Cmd::Check { id: Some(id) }) if id == "openai")
        );
        assert!(matches!(
            parse(&["update"]),
            Ok(Cmd::Update { dry_run: false })
        ));
        assert!(matches!(
            parse(&["update", "--dry-run"]),
            Ok(Cmd::Update { dry_run: true })
        ));
        assert!(parse(&["enable"]).is_err(), "an id is required");
    }

    #[test]
    fn engines_add_takes_a_registry_agent_or_a_local_command_with_its_flags() {
        let Ok(Cmd::Engines {
            action:
                EnginesCmd::Add {
                    id,
                    registry,
                    flags,
                },
        }) = parse(&[
            "engines",
            "add",
            "mine",
            "--registry",
            "claude-code",
            "--env",
            "A=1",
            "--env",
            "B=two=2",
            "--arg",
            "-x",
            "--route",
            "anthropic/claude-sonnet-5-5",
            "--version",
            "1.2.3",
            "--label",
            "Mine",
        ])
        else {
            panic!("engines add parses");
        };
        assert_eq!(id, "mine");
        assert_eq!(registry.as_deref(), Some("claude-code"));
        assert_eq!(flags.env, vec!["A=1".to_string(), "B=two=2".to_string()]);
        assert_eq!(
            flags.args,
            vec!["-x".to_string()],
            "a hyphen argument is a value"
        );
        assert_eq!(flags.version.as_deref(), Some("1.2.3"));
        assert_eq!(flags.label.as_deref(), Some("Mine"));

        let body = engine_body(&id, registry.as_deref(), &flags).expect("a valid engine");
        assert_eq!(body["engine"], json!("acp:claude-code"));
        assert_eq!(body["env"], json!({ "A": "1", "B": "two=2" }));
        assert_eq!(body["route"], json!("anthropic/claude-sonnet-5-5"));
        assert_eq!(body["args"], json!(["-x"]));

        // A local program: --command, no registry.
        assert!(matches!(
            parse(&["engines", "add", "local", "--command", "/usr/bin/tool"]),
            Ok(Cmd::Engines {
                action: EnginesCmd::Add { registry: None, .. }
            })
        ));
        let local = EngineFlags {
            command: Some("/usr/bin/tool".into()),
            ..Default::default()
        };
        assert_eq!(
            engine_body("local", None, &local).unwrap()["engine"],
            json!("acp-local")
        );
        // Neither or both are refused.
        assert!(engine_body("x", None, &EngineFlags::default()).is_err());
        assert!(engine_body("x", Some("a"), &local).is_err());
    }

    #[test]
    fn engines_set_and_remove_parse_and_only_the_flags_given_change() {
        assert!(matches!(
            parse(&[
                "engines",
                "set",
                "mine",
                "--command",
                "/bin/x",
                "--arg",
                "a",
                "--arg",
                "b"
            ]),
            Ok(Cmd::Engines {
                action: EnginesCmd::Set { .. }
            })
        ));
        assert!(matches!(
            parse(&["engines", "remove", "mine", "--yes"]),
            Ok(Cmd::Engines {
                action: EnginesCmd::Remove { yes: true, .. }
            })
        ));
        let one = EngineFlags {
            version: Some("latest".into()),
            ..Default::default()
        };
        assert_eq!(
            engine_edit_body(&one).unwrap(),
            json!({ "version": "latest" })
        );
        let cleared = EngineFlags {
            route: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            engine_edit_body(&cleared).unwrap(),
            json!({ "route": null })
        );
        assert!(
            engine_edit_body(&EngineFlags::default()).is_err(),
            "nothing to change is refused"
        );
        let bad_env = EngineFlags {
            env: vec!["1X=y".into()],
            ..Default::default()
        };
        assert!(engine_edit_body(&bad_env).is_err());
        assert!(env_map(&["NOEQUALS".into()]).is_err());
        assert!(checked_id("my-engine_2").is_ok());
        assert!(checked_id("My Engine").is_err());
    }

    #[test]
    fn registry_search_takes_a_query_or_none_and_refresh() {
        assert!(matches!(
            parse(&["registry", "search", "claude", "--refresh"]),
            Ok(Cmd::Registry { action: RegistryCmd::Search { query, refresh: true } }) if query == "claude"
        ));
        assert!(matches!(
            parse(&["registry", "search"]),
            Ok(Cmd::Registry { action: RegistryCmd::Search { query, refresh: false } }) if query.is_empty()
        ));
        let agent =
            json!({ "id": "claude-code", "name": "Claude Code", "description": "A coding agent" });
        assert!(matches_agent(&agent, "code"));
        assert!(matches_agent(&agent, "CLAUDE"));
        assert!(!matches_agent(&agent, "gemini"));
        assert!(matches_agent(&agent, ""));
    }

    #[test]
    fn favourite_takes_provider_slash_model_and_edits_the_list_once() {
        assert!(matches!(
            parse(&["favourite", "add", "openai/gpt-x"]),
            Ok(Cmd::Favourite { action: FavouriteCmd::Add { model } }) if model == "openai/gpt-x"
        ));
        assert!(matches!(
            parse(&["favourite", "remove", "openai/gpt-x"]),
            Ok(Cmd::Favourite {
                action: FavouriteCmd::Remove { .. }
            })
        ));
        assert!(
            favourite_key("openai/gpt-x@high").is_err(),
            "no effort in a favourite"
        );
        assert!(favourite_key("gpt-x").is_err());

        let mut root = Map::new();
        edit_favourites(&mut root, "a/b", true);
        edit_favourites(&mut root, "a/b", true);
        edit_favourites(&mut root, "c/d", true);
        assert_eq!(root["favourite_models"], json!(["a/b", "c/d"]));
        edit_favourites(&mut root, "a/b", false);
        assert_eq!(root["favourite_models"], json!(["c/d"]));
    }

    #[test]
    fn usage_providers_takes_its_subcommands_and_reads_the_key_from_stdin_only() {
        assert!(matches!(
            parse(&[
                "usage-providers",
                "add",
                "--url",
                "https://hub.example",
                "--label",
                "L",
                "--key-stdin"
            ]),
            Ok(Cmd::UsageProviders {
                action: UsageCmd::Add {
                    key_stdin: true,
                    ..
                }
            })
        ));
        assert!(matches!(
            parse(&["usage-providers", "list"]),
            Ok(Cmd::UsageProviders {
                action: UsageCmd::List
            })
        ));
        assert!(matches!(
            parse(&["usage-providers", "usage", "hub1"]),
            Ok(Cmd::UsageProviders { action: UsageCmd::Usage { id } }) if id == "hub1"
        ));
        assert!(matches!(
            parse(&["usage-providers", "remove", "hub1", "--yes"]),
            Ok(Cmd::UsageProviders {
                action: UsageCmd::Remove { yes: true, .. }
            })
        ));
        assert!(
            parse(&[
                "usage-providers",
                "add",
                "--url",
                "https://h",
                "--key",
                "secret"
            ])
            .is_err(),
            "a key is never an argument"
        );
    }

    #[test]
    fn settings_health_interval_takes_seconds_or_shows_them() {
        assert!(matches!(
            parse(&["settings", "health-interval", "300"]),
            Ok(Cmd::Settings {
                action: SettingsCmd::HealthInterval { seconds: Some(300) }
            })
        ));
        assert!(matches!(
            parse(&["settings", "health-interval"]),
            Ok(Cmd::Settings {
                action: SettingsCmd::HealthInterval { seconds: None }
            })
        ));
        assert!(parse(&["settings", "health-interval", "soon"]).is_err());
    }

    #[test]
    fn api_config_keeps_what_the_hub_lists_and_the_flag_it_is_given() {
        let listed = json!({
            "id": "openai", "kind": "openai", "label": "Work", "accent": "#5cc8ff", "enabled": false,
            "base_url": "https://api.openai.com/v1", "auth": { "secret": "openai" },
            "models": ["gpt-x"], "extra_headers": { "X-Team": "[redacted]" }, "extra_body": { "k": 1 },
            "credential": "secret", "health": { "status": "ok" }
        });
        let mut body = api_config(&listed);
        assert_eq!(body["enabled"], json!(false));
        assert_eq!(body["label"], json!("Work"));
        assert_eq!(body["extra_body"], json!({ "k": 1 }));
        assert!(
            body.get("credential").is_none(),
            "the listing's own fields do not go back"
        );
        body["enabled"] = json!(true);
        assert_eq!(body["enabled"], json!(true));
        assert_eq!(health_text(&listed["health"]), "ok");
        assert_eq!(
            health_text(&json!({ "status": "crit", "message": "Not authenticated" })),
            "crit  Not authenticated"
        );
    }

    #[test]
    fn a_version_is_pinned_latest_or_dotted_and_the_runtime_body_clears_what_is_empty() {
        assert_eq!(version_ok("latest").unwrap(), "latest");
        assert_eq!(version_ok("1.2.3").unwrap(), "1.2.3");
        assert!(version_ok("soon").is_err());
        assert!(version_ok("1..2").is_err());
        assert!(version_ok("").is_err());
        let body = runtime_body("pinned", "", "", &["--x".into()], &["A=1".into()]).unwrap();
        assert_eq!(body["home"], Value::Null);
        assert_eq!(body["route"], Value::Null);
        assert_eq!(body["args"], json!(["--x"]));
        assert_eq!(body["env"], json!({ "A": "1" }));
        assert!(runtime_body("pinned", "", "", &[], &["BAD".into()]).is_err());
        assert!(
            runtime_body("latest", "", "p", &[], &[]).is_err(),
            "a route is provider/model"
        );
        assert_eq!(
            usage_body("https://h", "k", Some("L")),
            json!({ "kind": "cliproxyapi", "url": "https://h", "key": "k", "label": "L" })
        );
    }
}
