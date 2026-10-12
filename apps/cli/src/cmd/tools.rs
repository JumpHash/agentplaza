//! `agentplaza tools`: Tools and connectors.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::cmd::models::{confirm, expect_stdin_marker, items, joined, read_secret_from_stdin, split_list};
use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The connectors: outside tools the agents may be lent, and whether each is set up.
    List,
    /// One connector: its credential fields, roles and actions.
    Show {
        /// The connector id.
        id: String,
    },
    /// Turn a connector on for the agents.
    Enable {
        /// The connector id.
        id: String,
    },
    /// Turn a connector off.
    Disable {
        /// The connector id.
        id: String,
    },
    /// Set the roles that may use a connector, comma-separated (empty = every role).
    Roles {
        /// The connector id.
        id: String,
        /// The roles, comma-separated.
        roles: String,
    },
    /// Store a connector's key (`--value -` reads it from stdin; it is never echoed), or clear it.
    Key {
        /// The connector id.
        id: String,
        /// The credential field, when the connector has more than one.
        #[arg(long)]
        field: Option<String>,
        /// `-`: read the key from stdin.
        #[arg(long)]
        value: Option<String>,
        /// Clear the stored key instead of setting it.
        #[arg(long)]
        clear: bool,
        /// Clear without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Test a connector's key with one call to it.
    Test {
        /// The connector id.
        id: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let list = ctx.client.get("/v1/connectors")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|c| {
                        vec![
                            ctx::s(c, "id").to_string(),
                            ctx::s(c, "label").to_string(),
                            ctx::s(c, "kind").to_string(),
                            ctx::s(c, "category").to_string(),
                            yes_no(c, "configured"),
                            yes_no(c, "enabled"),
                            roles_text(c),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "label", "kind", "category", "set up", "enabled", "roles"], &rows);
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let connector = find(ctx, &id)?;
            ctx.out(&connector, print_connector);
            Ok(())
        }
        Cmd::Enable { id } => set_enabled(ctx, &id, true),
        Cmd::Disable { id } => set_enabled(ctx, &id, false),
        Cmd::Roles { id, roles } => {
            let roles = split_list(&roles);
            let shown = if roles.is_empty() { "every role".to_string() } else { roles.join(", ") };
            ctx.client.put(&format!("/v1/connectors/{}", ctx::seg(&id)), &json!({ "roles": roles }))?;
            ctx.out(&json!({ "id": id, "roles": shown }), |v| {
                println!("{}: roles set to {}", ctx::s(v, "id"), ctx::s(v, "roles"));
            });
            Ok(())
        }
        Cmd::Key { id, field, value, clear, yes } => {
            let path = format!("/v1/connectors/{}/secret", ctx::seg(&id));
            let done = if clear {
                if value.is_some() {
                    return Err("--clear takes no --value".into());
                }
                confirm(&format!("Clear the stored key of {id}."), yes)?;
                let query = field.as_deref().map(|f| format!("?field={}", ctx::seg(f))).unwrap_or_default();
                ctx.client.delete(&format!("{path}{query}"))?;
                format!("cleared the stored key of {id}")
            } else {
                let marker = value.ok_or("give --value - (the key on stdin), or --clear")?;
                expect_stdin_marker(&marker)?;
                let key = read_secret_from_stdin()?;
                let mut body = json!({ "value": key });
                if let Some(f) = &field {
                    body["field"] = json!(f);
                }
                ctx.client.put(&path, &body)?;
                format!("stored the key of {id}")
            };
            ctx.out(&json!({ "id": id, "done": done }), |v| println!("{}", ctx::s(v, "done")));
            Ok(())
        }
        Cmd::Test { id } => {
            let report = ctx.client.post(&format!("/v1/connectors/{}/test", ctx::seg(&id)), &json!({}))?;
            let ok = report.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let detail = ctx::s(&report, "detail").to_string();
            ctx.out(&report, |r| {
                let word = if ok { "ok" } else { "failed" };
                println!("{word}  {} ms  {}", ctx::text(r, "latency_ms"), ctx::s(r, "detail"));
            });
            if ok {
                Ok(())
            } else {
                Err(format!("the test of {id} failed: {detail}"))
            }
        }
    }
}

/// One connector from the list (the hub has no single-connector read).
fn find(ctx: &Ctx, id: &str) -> Result<Value, String> {
    let list = ctx.client.get("/v1/connectors")?;
    items(&list)
        .find(|c| ctx::s(c, "id") == id)
        .cloned()
        .ok_or_else(|| format!("no connector {id}: see `agentplaza tools list`"))
}

fn set_enabled(ctx: &Ctx, id: &str, on: bool) -> Result<(), String> {
    ctx.client.put(&format!("/v1/connectors/{}", ctx::seg(id)), &json!({ "enabled": on }))?;
    let word = if on { "enabled" } else { "disabled" };
    ctx.out(&json!({ "id": id, "enabled": on }), |_| println!("{id} {word}"));
    Ok(())
}

fn yes_no(v: &Value, key: &str) -> String {
    if v.get(key).and_then(Value::as_bool).unwrap_or(false) {
        "yes".into()
    } else {
        "no".into()
    }
}

/// A connector's roles, or "all" when every role may use it.
fn roles_text(c: &Value) -> String {
    let roles = joined(c.get("roles"));
    if roles.is_empty() {
        "all".into()
    } else {
        roles
    }
}

fn print_connector(c: &Value) {
    ctx::fields(&[
        ("id", ctx::s(c, "id").to_string()),
        ("label", ctx::s(c, "label").to_string()),
        ("kind", ctx::s(c, "kind").to_string()),
        ("category", ctx::s(c, "category").to_string()),
        ("description", ctx::s(c, "description").to_string()),
        ("docs", ctx::s(c, "docs_url").to_string()),
        ("set up", yes_no(c, "configured")),
        ("enabled", yes_no(c, "enabled")),
        ("roles", roles_text(c)),
    ]);
    println!();
    let fields: Vec<Vec<String>> = c
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|f| {
            let status = if yes_no(f, "set") == "yes" {
                format!("set from {}", ctx::s(f, "source"))
            } else {
                "not set".into()
            };
            vec![
                ctx::s(f, "name").to_string(),
                ctx::s(f, "label").to_string(),
                if yes_no(f, "required") == "yes" { "required".into() } else { "optional".into() },
                status,
                ctx::s(f, "env").to_string(),
            ]
        })
        .collect();
    ctx::table(&["field", "label", "need", "status", "env"], &fields);
    println!();
    let actions: Vec<Vec<String>> = c
        .get("actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|a| vec![ctx::s(a, "name").to_string(), ctx::s(a, "risk").to_string(), ctx::s(a, "description").to_string()])
        .collect();
    ctx::table(&["action", "risk", "description"], &actions);
}
