//! `agentplaza hub`: saved hubs. `login` checks the hub answers the owner token, then saves it
//! (in the platform's config directory, private to this user) so later commands need no flags.
//! The identity a saved hub presents (its TLS certificate) can be pinned by trust on first use, as
//! the desktop app and the web console do: `trust` pins it, `forget-pin` forgets it.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use serde_json::{Map, Value, json};

use crate::ctx::{self, Client, Config, SavedHub, confirm};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Sign in to a hub and save it (the first saved hub becomes the default). Before the owner token
    /// is sent, the hub's identity is checked: a pinned one must be the one it presents; on first use
    /// its fingerprint is shown and asked about, then pinned.
    Login {
        /// The hub's URL, e.g. https://plaza.example:7700 (default: the local hub).
        url: Option<String>,
        /// The owner token (`apid` keeps it in its state directory; with no URL, the local hub's is read there).
        #[arg(long, env = "APZ_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// The hub certificate's SHA-256 pin, hex (for a hub with its own certificate).
        #[arg(long)]
        pin: Option<String>,
        /// A short name for it (default: its host).
        #[arg(long)]
        name: Option<String>,
        /// Trust the hub's identity on first use without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Forget a saved hub.
    Logout { name: String },
    /// The saved hubs; the default is marked.
    List,
    /// Make a saved hub the default.
    Use { name: String },
    /// Check the default (or `--hub`) hub answers and who it is.
    Status {
        #[arg(long, env = "APZ_HUB")]
        hub: Option<String>,
    },
    /// Make a new owner token: the current one stops working at once. The new one is shown once and
    /// saved in the hub's saved entry.
    RotateToken {
        /// Rotate without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Forget the hub identity pinned for a saved hub (its certificate). The next sign-in asks again.
    ForgetPin {
        /// The saved hub's name (default: `--hub`, else the default hub).
        #[arg(value_name = "HUB")]
        name: Option<String>,
    },
    /// Pin the identity a saved hub presents now, after showing its fingerprint and asking.
    Trust {
        /// The saved hub's name (default: `--hub`, else the default hub).
        #[arg(value_name = "HUB")]
        name: Option<String>,
        /// Trust without asking.
        #[arg(long)]
        yes: bool,
    },
}

/// `hub` runs with the global `--hub`, `--token` and `--pin` (the hub the owner-token and pin
/// commands act on), as the other commands that talk to a hub do.
pub fn run(
    cmd: Cmd,
    json_out: bool,
    hub: Option<String>,
    token: Option<String>,
    pin: Option<String>,
) -> Result<(), String> {
    let mut config = Config::load();
    match cmd {
        Cmd::Login {
            url,
            token,
            pin,
            name,
            yes,
        } => {
            let url = url
                .unwrap_or_else(|| ctx::LOCAL_HUB.to_string())
                .trim_end_matches('/')
                .to_string();
            let token = token.or_else(|| {
                (url == ctx::LOCAL_HUB)
                    .then(ctx::local_owner_token)
                    .flatten()
            });
            let name = name.unwrap_or_else(|| host_of(&url));
            // The saved entry this sign-in writes, by name: its hex pin and pinned identity apply as
            // `ctx::resolve` applies them, and a flag overrides the saved pin.
            let saved = config.hubs.get(&name).cloned();
            let pin = pin.or_else(|| saved.as_ref().and_then(|s| s.pin.clone()));
            let pin_bytes = pin.as_deref().map(ctx::parse_pin).transpose()?;
            let pinned = ctx::pinned_identity(&Config::path(), &name);
            // Checked before the owner token goes out.
            let trust = check_identity(&name, pin_bytes.is_some(), pinned.as_deref(), || {
                Client::new(&url, None, None).get("/v1/hub/identity")
            })?;
            if let Trust::FirstUse {
                fingerprint,
                identity,
            } = &trust
            {
                if !json_out {
                    let who = ctx::s(identity, "name");
                    println!(
                        "{} {} at {url}",
                        if who.is_empty() { "unnamed hub" } else { who },
                        ctx::s(identity, "version")
                    );
                    println!("certificate {fingerprint}");
                }
                if !confirm(
                    yes,
                    "Trust this identity? Later sign-ins refuse any other identity until the pin is forgotten.",
                )? {
                    println!("not signed in");
                    return Ok(());
                }
            }
            let client = Client::new(&url, pin_bytes, token.clone());
            client
                .get("/v1/auth/check")
                .map_err(|e| format!("{url} did not accept the sign-in: {e}"))?;
            config.hubs.insert(
                name.clone(),
                SavedHub {
                    url: url.clone(),
                    token,
                    pin,
                    mcp_token: None,
                },
            );
            if config.default.is_none() {
                config.default = Some(name.clone());
            }
            config.save()?;
            if let Trust::FirstUse { fingerprint, .. } = &trust {
                // The entry `hub trust` writes: the saved hub's `pinned` fingerprint and the day.
                let mut doc = load_doc();
                if let Some(saved) = saved_hub(&mut doc, &name) {
                    saved.insert(
                        "pinned".into(),
                        json!({ "fingerprint": fingerprint, "since": utc_day(now_ms()) }),
                    );
                }
                save_doc(&doc)?;
            }
            println!(
                "signed in to {url} as `{name}`{}",
                if config.default.as_deref() == Some(&name) {
                    " (default)"
                } else {
                    ""
                }
            );
        }
        Cmd::Logout { name } => {
            if config.hubs.remove(&name).is_none() {
                return Err(format!("no saved hub `{name}`"));
            }
            if config.default.as_deref() == Some(&name) {
                config.default = config.hubs.keys().next().cloned();
            }
            config.save()?;
            println!("forgot `{name}`");
        }
        Cmd::List => {
            if json_out {
                let hubs: serde_json::Map<String, serde_json::Value> = config
                    .hubs
                    .iter()
                    .map(|(n, h)| (n.clone(), json!({ "url": h.url, "pinned": h.pin.is_some(), "token": h.token.is_some() })))
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({ "default": config.default, "hubs": hubs })
                    )
                    .unwrap_or_default()
                );
                return Ok(());
            }
            let rows: Vec<Vec<String>> = config
                .hubs
                .iter()
                .map(|(n, h)| {
                    vec![
                        if config.default.as_deref() == Some(n) {
                            format!("* {n}")
                        } else {
                            format!("  {n}")
                        },
                        h.url.clone(),
                        if h.pin.is_some() {
                            "pinned".into()
                        } else {
                            String::new()
                        },
                    ]
                })
                .collect();
            ctx::table(&["hub", "url", "tls"], &rows);
        }
        Cmd::Use { name } => {
            if !config.hubs.contains_key(&name) {
                return Err(format!("no saved hub `{name}` (see `agentplaza hub list`)"));
            }
            config.default = Some(name.clone());
            config.save()?;
            println!("default hub: {name}");
        }
        Cmd::Status { hub } => {
            let ctx = ctx::resolve(hub, None, None, json_out)?;
            let check = ctx.client.get("/v1/auth/check")?;
            ctx.out(&check, |_| println!("{}: signed in", ctx.client.base()));
        }
        Cmd::RotateToken { yes } => {
            // The saved entry the new token goes into: the one this command reaches by name, flag or default.
            let saved = saved_name(&config, None, hub.clone()).ok();
            let ctx = ctx::resolve(hub, token, pin, json_out)?;
            if !confirm(
                yes,
                "Rotate the owner token? The current one stops working at once, and every signed-in browser and app must sign in again.",
            )? {
                println!("not rotated");
                return Ok(());
            }
            let made = ctx.client.post("/v1/hub/owner-token/rotate", &json!({}))?;
            let new_token = ctx::s(&made, "token").to_string();
            if new_token.is_empty() {
                return Err("the hub answered without a token".into());
            }
            // Shown first: a save that fails must not take the only copy of the token with it.
            ctx.out(&made, |_| {
                println!("new owner token, shown once:\n  {new_token}")
            });
            match saved {
                Some(name) => {
                    if let Some(hub) = config.hubs.get_mut(&name) {
                        hub.token = Some(new_token);
                    }
                    config.save()?;
                    if !json_out {
                        println!("saved in `{name}`");
                    }
                }
                None if !json_out => println!(
                    "this hub has no saved entry, so nothing was saved: sign in with `agentplaza hub login <url> --token <new token>`"
                ),
                None => {}
            }
        }
        Cmd::ForgetPin { name } => {
            let name = saved_name(&config, name, hub)?;
            let mut doc = load_doc();
            let Some(saved) = saved_hub(&mut doc, &name) else {
                return Err(format!("no saved hub `{name}`"));
            };
            if saved.remove("pinned").is_none() {
                println!("`{name}` has no pinned identity");
                return Ok(());
            }
            save_doc(&doc)?;
            if json_out {
                println!("{}", json!({ "hub": name, "pinned": null }));
            } else {
                println!("forgot the identity pinned for `{name}`: the next sign-in asks again");
            }
        }
        Cmd::Trust { name, yes } => {
            let name = saved_name(&config, name, hub)?;
            let hub = &config.hubs[&name];
            let pin_bytes = hub.pin.as_deref().map(ctx::parse_pin).transpose()?;
            let identity = Client::new(&hub.url, pin_bytes, None).get("/v1/hub/identity")?;
            let url = hub.url.clone();
            let Some(fingerprint) = identity
                .get("fingerprint")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return Err(format!(
                    "{url} has no TLS certificate to pin: it answers over plain HTTP"
                ));
            };
            let mut doc = load_doc();
            let previous = doc
                .pointer(&format!(
                    "/hubs/{}/pinned/fingerprint",
                    name.replace('~', "~0").replace('/', "~1")
                ))
                .and_then(Value::as_str)
                .map(str::to_string);
            let who = ctx::s(&identity, "name");
            if !json_out {
                println!(
                    "{} {} at {url}",
                    if who.is_empty() { "unnamed hub" } else { who },
                    ctx::s(&identity, "version")
                );
                println!("certificate {fingerprint}");
                if let Some(old) = &previous {
                    println!("this replaces the pinned certificate {old}");
                }
            }
            if !confirm(
                yes,
                "Trust this identity? Later sign-ins refuse any other identity until the pin is forgotten.",
            )? {
                println!("not trusted");
                return Ok(());
            }
            let since = utc_day(now_ms());
            if let Some(saved) = saved_hub(&mut doc, &name) {
                saved.insert(
                    "pinned".into(),
                    json!({ "fingerprint": fingerprint, "since": since }),
                );
            }
            save_doc(&doc)?;
            if json_out {
                println!(
                    "{}",
                    json!({ "hub": name, "fingerprint": fingerprint, "since": since })
                );
            } else {
                println!("trusting the identity of `{name}` from {since}");
            }
        }
    }
    Ok(())
}

/// What `hub login` learns of the hub's identity before it sends the owner token.
#[derive(Debug, PartialEq)]
enum Trust {
    /// A hex pin is in use: TLS checks the certificate against it before the token goes out.
    Pin,
    /// The hub presents the identity pinned for the saved hub.
    Pinned,
    /// Nothing is pinned yet: the identity is shown and asked about, then pinned.
    FirstUse {
        fingerprint: String,
        identity: Value,
    },
    /// Nothing is pinned and the hub has no certificate to pin (it answers over plain HTTP).
    Plain,
}

/// The identity check of `hub login`, as `ctx::resolve` makes it. A hex pin is checked by TLS and
/// skips the identity call. A pinned identity must be the one the hub presents. With no pin, the
/// presented identity is the one the user is asked to trust. `ask` fetches the identity (`GET
/// /v1/hub/identity`, unauthenticated); its failure refuses the sign-in.
fn check_identity(
    name: &str,
    hex_pin: bool,
    pinned: Option<&str>,
    ask: impl FnOnce() -> Result<Value, String>,
) -> Result<Trust, String> {
    if hex_pin {
        return Ok(Trust::Pin);
    }
    let identity = ask()?;
    let presented = fingerprint_of(&identity);
    match pinned {
        Some(pinned) => {
            ctx::compare_identity(name, pinned, presented.as_deref())?;
            Ok(Trust::Pinned)
        }
        None => Ok(match presented {
            Some(fingerprint) => Trust::FirstUse {
                fingerprint,
                identity,
            },
            None => Trust::Plain,
        }),
    }
}

/// The certificate fingerprint in a hub identity, when it has one.
fn fingerprint_of(identity: &Value) -> Option<String> {
    identity
        .get("fingerprint")
        .and_then(Value::as_str)
        .filter(|f| !f.is_empty())
        .map(str::to_string)
}

/// The saved hub a command means: the name it was given, else the global `--hub` (a saved name),
/// else the default hub. An unsaved URL or no hub at all is an error.
fn saved_name(
    config: &Config,
    named: Option<String>,
    flag: Option<String>,
) -> Result<String, String> {
    let name = named
        .or(flag)
        .or_else(|| config.default.clone())
        .ok_or_else(|| "no saved hub: sign in with `agentplaza hub login <url>`".to_string())?;
    if config.hubs.contains_key(&name) {
        Ok(name)
    } else {
        Err(format!("no saved hub `{name}` (see `agentplaza hub list`)"))
    }
}

/// `cli.json` as JSON. The CLI's own config type does not model the `pinned` entry the desktop app
/// writes, so the pins are edited in the file itself and every other key is kept as it is.
fn load_doc() -> Value {
    fs::read_to_string(Config::path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_doc(doc: &Value) -> Result<(), String> {
    let path = Config::path();
    if let Some(dir) = path.parent() {
        apz_os::private::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    apz_os::private::write(&path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// A saved hub's entry in the document, as an object to edit.
fn saved_hub<'a>(doc: &'a mut Value, name: &str) -> Option<&'a mut Map<String, Value>> {
    doc.get_mut("hubs")?.get_mut(name)?.as_object_mut()
}

/// Epoch milliseconds as the UTC day `YYYY-MM-DD` (the day an identity was pinned).
fn utc_day(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `https://plaza.example:7700` → `plaza.example`.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    host.trim_matches(['[', ']']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(names: &[&str], default: Option<&str>) -> Config {
        let mut config = Config {
            default: default.map(str::to_string),
            ..Config::default()
        };
        for name in names {
            config.hubs.insert(
                name.to_string(),
                SavedHub {
                    url: format!("https://{name}.example:7700"),
                    token: None,
                    pin: None,
                    mcp_token: None,
                },
            );
        }
        config
    }

    #[test]
    fn the_named_hub_wins_then_the_flag_then_the_default() {
        let config = config_with(&["home", "work"], Some("home"));
        assert_eq!(
            saved_name(&config, Some("work".into()), Some("home".into())).unwrap(),
            "work"
        );
        assert_eq!(
            saved_name(&config, None, Some("work".into())).unwrap(),
            "work"
        );
        assert_eq!(saved_name(&config, None, None).unwrap(), "home");
    }

    #[test]
    fn an_unsaved_hub_or_no_hub_at_all_is_refused() {
        let config = config_with(&["home"], None);
        assert!(
            saved_name(&config, None, None)
                .unwrap_err()
                .contains("hub login")
        );
        assert!(
            saved_name(&config, None, Some("https://elsewhere.example:7700".into()))
                .unwrap_err()
                .contains("no saved hub")
        );
    }

    #[test]
    fn a_pin_day_is_the_utc_date() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(1_791_811_353_718), "2026-10-12");
        assert_eq!(utc_day(1_791_811_353_718 + 86_400_000), "2026-10-13");
    }

    #[test]
    fn a_hex_pin_skips_the_identity_call() {
        let trust = check_identity("home", true, Some("fp-a"), || -> Result<Value, String> {
            panic!("the identity is not asked for")
        });
        assert_eq!(trust, Ok(Trust::Pin));
    }

    #[test]
    fn a_pinned_identity_must_be_the_one_presented() {
        assert_eq!(
            check_identity("home", false, Some("fp-a"), || Ok(
                json!({ "fingerprint": "fp-a" })
            )),
            Ok(Trust::Pinned)
        );
        let other = check_identity("home", false, Some("fp-a"), || {
            Ok(json!({ "fingerprint": "fp-b" }))
        })
        .unwrap_err();
        assert_eq!(
            other,
            "the hub's identity changed: pinned fp-a, presented fp-b; run `agentplaza hub forget-pin home` if you expect this"
        );
        let none = check_identity("home", false, Some("fp-a"), || {
            Ok(json!({ "name": "home" }))
        })
        .unwrap_err();
        assert!(none.contains("presented no certificate"), "{none}");
    }

    #[test]
    fn a_first_sign_in_asks_about_the_presented_identity() {
        let identity = json!({ "fingerprint": "fp-a", "name": "home", "version": "0.0.8" });
        let trust = check_identity("home", false, None, || Ok(identity.clone())).unwrap();
        assert_eq!(
            trust,
            Trust::FirstUse {
                fingerprint: "fp-a".into(),
                identity
            }
        );
    }

    #[test]
    fn a_hub_without_a_certificate_has_nothing_to_pin() {
        assert_eq!(
            check_identity("local", false, None, || Ok(json!({ "name": "local" }))),
            Ok(Trust::Plain)
        );
    }

    #[test]
    fn an_identity_that_cannot_be_read_refuses_the_sign_in() {
        assert_eq!(
            check_identity("home", false, Some("fp-a"), || Err(
                "connection refused".into()
            )),
            Err("connection refused".to_string())
        );
        assert_eq!(
            check_identity("home", false, None, || Err("connection refused".into())),
            Err("connection refused".to_string())
        );
    }

    #[test]
    fn host_names_drop_the_scheme_port_and_path() {
        assert_eq!(host_of("https://plaza.example:7700/x"), "plaza.example");
        assert_eq!(host_of("http://[::1]:7700"), "::1");
    }
}
