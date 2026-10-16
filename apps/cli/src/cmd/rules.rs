//! `agentplaza rules`: The plaza's laws: what each one allows or forbids, the infractions the
//! laws stopped, the owner's own rule sets (JHRL sources), and the switches on the built-in laws.

use std::io::Read;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

/// The tier the hub gives a constitutional law (`crates/apid/src/laws.rs`).
const CONSTITUTION: &str = "constitution";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The laws, with what each one allows or forbids.
    List {
        /// Only the laws that permit, or only those that forbid.
        #[arg(long, value_parser = ["permit", "forbid"])]
        effect: Option<String>,
    },
    /// One law, explained: what it covers, its wording and why it exists.
    Show {
        /// The law id.
        id: String,
    },
    /// The infractions: actions the laws stopped, newest first.
    Infractions {
        /// How many to fetch.
        #[arg(long, default_value_t = 200)]
        limit: u32,
        /// Only the infractions of this law.
        #[arg(long)]
        law: Option<String>,
    },
    /// The owner's rules: the rule sets the owner wrote, whether each is on, and what it declares.
    Owner,
    /// The recorded refusals of one law, newest first.
    Violations {
        /// The law id.
        law: String,
        /// How many to fetch.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// A new owner rule from a JHRL source file (`-` reads stdin). It applies only if it compiles.
    Create {
        /// The rule's title.
        #[arg(long)]
        title: String,
        /// The JHRL source file.
        #[arg(long)]
        file: String,
    },
    /// Change an owner rule: its title, its source, or both. A new source applies only if it compiles.
    Edit {
        /// The rule's id.
        id: String,
        /// A new title.
        #[arg(long)]
        title: Option<String>,
        /// A new JHRL source file (`-` reads stdin).
        #[arg(long)]
        file: Option<String>,
    },
    /// Turn an owner rule on: its laws apply from now on.
    Enable {
        /// The rule's id.
        id: String,
    },
    /// Turn an owner rule off: its laws stop applying.
    Disable {
        /// The rule's id.
        id: String,
    },
    /// Delete an owner rule and the laws it declares. Asks first unless --yes.
    Delete {
        /// The rule's id.
        id: String,
        /// Delete without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Dry-run a JHRL source file: its diagnostics, the laws it declares, and the recent calls it would
    /// refuse. Nothing is applied.
    Check {
        /// The JHRL source file (`-` reads stdin).
        #[arg(long)]
        file: String,
        /// The owner rule this source would replace.
        #[arg(long)]
        id: Option<String>,
    },
    /// Switch a built-in law on or off. Off needs --reason; a constitutional law needs --confirm with its id.
    Override {
        /// The law id.
        law: String,
        /// on or off.
        #[arg(value_parser = ["on", "off"])]
        state: String,
        /// Why the law is switched off (kept on record).
        #[arg(long, default_value = "")]
        reason: String,
        /// For a constitutional law switched off: its id, typed again.
        #[arg(long)]
        confirm: Option<String>,
    },
    /// Strict permits: on refuses an operation that has permit laws unless one of them allows it.
    Strict {
        /// on or off.
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A list of strings joined with ", " ("-" when there are none).
fn list_of(v: &Value) -> String {
    let items: Vec<&str> = array(v).iter().filter_map(Value::as_str).collect();
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(", ")
    }
}

/// The effects of the laws one owner rule declares ("forbid", "permit", or both), or "-".
fn effects_of(laws: &Value, rule_id: &str) -> String {
    let mut effects: Vec<&str> = array(laws)
        .iter()
        .filter(|l| ctx::s(l, "rule_id") == rule_id)
        .map(|l| ctx::s(l, "effect"))
        .collect();
    effects.sort_unstable();
    effects.dedup();
    list_of(&json!(effects))
}

/// Infractions (or one law's refusals) as a table.
fn infraction_table(v: &Value) {
    let rows: Vec<Vec<String>> = array(v)
        .iter()
        .map(|i| {
            vec![
                ctx::when(&i["at_ms"]),
                ctx::s(i, "agent_name").to_string(),
                ctx::s(i, "law").to_string(),
                ctx::s(i, "tool").to_string(),
                ctx::s(i, "detail").to_string(),
            ]
        })
        .collect();
    ctx::table(&["at (ms)", "agent", "law", "tool", "detail"], &rows);
}

/// A title or reason as typed, trimmed; blank is refused.
fn non_blank(text: &str, what: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Err(format!("{what} is empty"))
    } else {
        Ok(trimmed.to_string())
    }
}

/// A JHRL source from a file (`-` reads stdin). A blank source is refused.
fn read_source(file: &str) -> Result<String, String> {
    let text = if file == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
        buf
    } else {
        std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?
    };
    if text.trim().is_empty() {
        return Err(format!("{file} is empty"));
    }
    Ok(text)
}

/// A switch-off of a constitutional law needs `confirm` set to its exact id (the hub answers 409
/// without it). Switching on, and any statute, need nothing extra.
fn override_confirm_ok(
    tier: &str,
    law: &str,
    disabled: bool,
    confirm: Option<&str>,
) -> Result<(), String> {
    if disabled && tier == CONSTITUTION && confirm != Some(law) {
        return Err(format!(
            "{law} is a constitutional law: to switch it off, pass --confirm {law} (its id, typed again)"
        ));
    }
    Ok(())
}

/// The settings document with strict permits set. `dev_models` is apid's own field, added on the way
/// out, so it is dropped before the settings are saved back.
fn settings_with_strict(settings: &Value, on: bool) -> Value {
    let mut next = settings.clone();
    if let Some(map) = next.as_object_mut() {
        map.remove("dev_models");
        let laws = map.entry("laws").or_insert_with(|| json!({}));
        if !laws.is_object() {
            *laws = json!({});
        }
        laws["strict_permits"] = json!(on);
    }
    next
}

/// Turns an owner rule on or off; its laws start or stop applying.
fn set_rule_enabled(ctx: &Ctx, id: &str, enabled: bool) -> Result<(), String> {
    let action = if enabled { "enable" } else { "disable" };
    let rule = ctx.client.post(
        &format!("/v1/laws/rules/{}/{action}", ctx::seg(id)),
        &json!({}),
    )?;
    ctx.out(&rule, |_| {
        println!("rule {id} {}", if enabled { "enabled" } else { "disabled" })
    });
    Ok(())
}

/// The dry run: the diagnostics when the source does not compile; else the laws it declares and the
/// recent calls it would have refused.
fn check_report(v: &Value) {
    if !v["ok"].as_bool().unwrap_or(false) {
        println!("does not compile:");
        let rows: Vec<Vec<String>> = array(&v["diagnostics"])
            .iter()
            .map(|d| {
                vec![
                    ctx::text(d, "file"),
                    ctx::text(d, "line"),
                    ctx::text(d, "column"),
                    ctx::s(d, "message").to_string(),
                ]
            })
            .collect();
        ctx::table(&["file", "line", "column", "message"], &rows);
        return;
    }
    println!("compiles; it declares: {}", list_of(&v["laws"]));
    println!();
    let rows: Vec<Vec<String>> = array(&v["would_refuse"])
        .iter()
        .map(|w| {
            vec![
                ctx::text(w, "cid"),
                ctx::s(w, "role").to_string(),
                ctx::s(w, "tool").to_string(),
                ctx::when(&w["at_ms"]),
                list_of(&w["laws"]),
            ]
        })
        .collect();
    println!("recent calls it would refuse: {}", rows.len());
    ctx::table(&["cid", "role", "tool", "at", "laws"], &rows);
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { effect } => {
            let mut v = ctx.client.get("/v1/laws")?;
            if let Some(effect) = effect
                && let Some(list) = v.as_array_mut()
            {
                list.retain(|l| ctx::s(l, "effect") == effect.as_str());
            }
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|l| {
                        vec![
                            ctx::s(l, "id").to_string(),
                            ctx::s(l, "effect").to_string(),
                            ctx::s(l, "severity").to_string(),
                            ctx::text(l, "refusals"),
                            ctx::s(l, "title").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "effect", "severity", "refusals", "title"], &rows);
            });
        }
        Cmd::Show { id } => {
            let v = ctx.client.get("/v1/laws")?;
            let law = array(&v)
                .iter()
                .find(|l| ctx::s(l, "id") == id.as_str())
                .cloned()
                .ok_or_else(|| format!("no law {id}"))?;
            ctx.out(&law, |l| {
                let ops: Vec<String> = array(l.get("ops").unwrap_or(&Value::Null))
                    .iter()
                    .map(|o| o.as_str().unwrap_or_default().to_string())
                    .collect();
                let conditions: Vec<String> = array(l.get("conditions").unwrap_or(&Value::Null))
                    .iter()
                    .map(|c| c.as_str().unwrap_or_default().to_string())
                    .collect();
                ctx::fields(&[
                    ("id", ctx::text(l, "id")),
                    ("title", ctx::text(l, "title")),
                    ("effect", ctx::text(l, "effect")),
                    ("severity", ctx::text(l, "severity")),
                    ("enforcement", ctx::text(l, "enforcement")),
                    (
                        "applies to",
                        format!("{} -> {}", ctx::s(l, "principal"), ctx::s(l, "resource")),
                    ),
                    ("operations", ops.join(", ")),
                    ("statement", ctx::text(l, "statement")),
                    ("in short", ctx::text(l, "explanation")),
                    ("why", ctx::text(l, "rationale")),
                    (
                        "conditions",
                        if conditions.is_empty() {
                            "none".to_string()
                        } else {
                            conditions.join("; ")
                        },
                    ),
                    ("file", ctx::text(l, "file")),
                    ("refusals", ctx::text(l, "refusals")),
                ]);
            });
        }
        Cmd::Infractions { limit, law } => {
            // With a law, the hub filters before its limit, so the newest rows of that law come back.
            let v = match law {
                Some(law) => ctx.client.get(&format!(
                    "/v1/laws/violations?law={}&limit={limit}",
                    ctx::seg(&law)
                ))?,
                None => ctx.client.get(&format!("/v1/infractions?limit={limit}"))?,
            };
            ctx.out(&v, infraction_table);
        }
        Cmd::Owner => {
            let rules = ctx.client.get("/v1/laws/rules")?;
            let laws = ctx.client.get("/v1/laws")?;
            ctx.out(&rules, |r| {
                let rows: Vec<Vec<String>> = array(r.get("rules").unwrap_or(&Value::Null))
                    .iter()
                    .map(|rule| {
                        let id = ctx::s(rule, "id");
                        let enabled = if rule["enabled"].as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        };
                        vec![
                            id.to_string(),
                            enabled.to_string(),
                            effects_of(&laws, id),
                            ctx::s(rule, "title").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "enabled", "effect", "title"], &rows);
            });
        }
        Cmd::Violations { law, limit } => {
            let v = ctx.client.get(&format!(
                "/v1/laws/violations?law={}&limit={limit}",
                ctx::seg(&law)
            ))?;
            ctx.out(&v, infraction_table);
        }
        Cmd::Create { title, file } => {
            let title = non_blank(&title, "the title")?;
            let source = read_source(&file)?;
            let made = ctx.client.post(
                "/v1/laws/rules",
                &json!({ "title": title, "source": source }),
            )?;
            ctx.out(&made, |r| {
                println!("rule {} created: {}", ctx::s(r, "id"), ctx::s(r, "title"))
            });
        }
        Cmd::Edit { id, title, file } => {
            let mut body = serde_json::Map::new();
            if let Some(title) = title {
                body.insert("title".into(), json!(non_blank(&title, "the title")?));
            }
            if let Some(file) = file {
                body.insert("source".into(), json!(read_source(&file)?));
            }
            if body.is_empty() {
                return Err("nothing to change: pass --title, --file or both".into());
            }
            let rule = ctx.client.put(
                &format!("/v1/laws/rules/{}", ctx::seg(&id)),
                &Value::Object(body),
            )?;
            ctx.out(&rule, |r| {
                println!("rule {} saved: {}", ctx::s(r, "id"), ctx::s(r, "title"))
            });
        }
        Cmd::Enable { id } => set_rule_enabled(ctx, &id, true)?,
        Cmd::Disable { id } => set_rule_enabled(ctx, &id, false)?,
        Cmd::Delete { id, yes } => {
            if !confirm(
                yes,
                &format!("Delete rule {id} and the laws it declares? They stop applying for good."),
            )? {
                println!("not deleted");
                return Ok(());
            }
            let deleted = ctx
                .client
                .delete(&format!("/v1/laws/rules/{}", ctx::seg(&id)))?;
            ctx.out(&deleted, |_| println!("rule {id} deleted"));
        }
        Cmd::Check { file, id } => {
            let mut body = json!({ "source": read_source(&file)? });
            if let Some(id) = id {
                body["id"] = json!(id);
            }
            let result = ctx.client.post("/v1/laws/check", &body)?;
            ctx.out(&result, check_report);
        }
        Cmd::Override {
            law,
            state,
            reason,
            confirm,
        } => {
            let disabled = state == "off";
            let catalog = ctx.client.get("/v1/laws")?;
            let tier = array(&catalog)
                .iter()
                .find(|l| ctx::s(l, "id") == law)
                .map(|l| ctx::s(l, "tier").to_string())
                .ok_or_else(|| format!("no law {law}"))?;
            override_confirm_ok(&tier, &law, disabled, confirm.as_deref())?;
            let reason = reason.trim().to_string();
            if disabled && reason.is_empty() {
                return Err(format!("say why {law} is switched off: pass --reason"));
            }
            let body = json!({ "disabled": disabled, "reason": reason, "confirm": confirm.filter(|_| disabled) });
            let made = ctx
                .client
                .put(&format!("/v1/laws/overrides/{}", ctx::seg(&law)), &body)?;
            ctx.out(&made, |_| {
                println!(
                    "{law} {}",
                    if disabled {
                        "switched off"
                    } else {
                        "switched on"
                    }
                )
            });
        }
        Cmd::Strict { state } => {
            let on = state == "on";
            let settings = ctx.client.get("/v1/settings")?;
            let saved = ctx
                .client
                .put("/v1/settings", &settings_with_strict(&settings, on))?;
            ctx.out(&saved, |_| {
                println!("strict permits {}", if on { "on" } else { "off" })
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constitutional_switch_off_needs_its_id_typed_again() {
        let law = "C-002b";
        assert!(override_confirm_ok("constitution", law, true, None).is_err());
        assert!(override_confirm_ok("constitution", law, true, Some("C-004a")).is_err());
        assert!(override_confirm_ok("constitution", law, true, Some(law)).is_ok());
        // Switching a constitutional law on needs nothing, and a statute never needs the id.
        assert!(override_confirm_ok("constitution", law, false, None).is_ok());
        assert!(override_confirm_ok("statute", "S-013d", true, None).is_ok());
    }

    #[test]
    fn settings_with_strict_sets_the_switch() {
        // The strict setting keeps the rest of the settings and drops dev_models.
        {
            let before = json!({ "dev_models": true, "laws": { "strict_permits": false }, "payments": { "approve_over": 5 } });
            let after = settings_with_strict(&before, true);
            assert!(after.get("dev_models").is_none());
            assert_eq!(after["laws"]["strict_permits"], json!(true));
            assert_eq!(after["payments"]["approve_over"], json!(5));
            // The settings it was given are not changed.
            assert_eq!(before["dev_models"], json!(true));
        }
        // The strict setting is added when the laws section is missing.
        {
            let after = settings_with_strict(&json!({ "dev_models": false }), false);
            assert!(after.get("dev_models").is_none());
            assert_eq!(after["laws"]["strict_permits"], json!(false));
        }
    }
}
