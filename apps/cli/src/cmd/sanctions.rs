//! `agentplaza sanctions`: The sanctions the laws' violations drew (warnings, fines, tool suspensions,
//! and the steps the owner decides), and lifting an active tool suspension.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The sanctions and the tool suspensions, newest first.
    List {
        /// Only one agent's sanctions (its cid).
        #[arg(long)]
        cid: Option<u64>,
        /// Only the sanctions of this law.
        #[arg(long)]
        law: Option<String>,
        /// How many sanctions to fetch.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Lift an active tool suspension before it runs out.
    Lift {
        /// The sanction's id, as `sanctions list` shows it (a tool suspension).
        id: u64,
        /// Why it is lifted (kept on the record).
        #[arg(long)]
        reason: String,
    },
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// What the owner did on a sanction: its decision, else "lifted" when it was lifted, else "".
fn owner_of(s: &Value) -> String {
    match s
        .get("owner_action")
        .and_then(|o| o.get("action"))
        .and_then(Value::as_str)
    {
        Some(action) => action.to_string(),
        None if s.get("lifted").is_some_and(|l| !l.is_null()) => "lifted".to_string(),
        None => String::new(),
    }
}

/// The sanctions and the suspensions, as two tables.
fn register(v: &Value) {
    let rows: Vec<Vec<String>> = array(&v["sanctions"])
        .iter()
        .map(|s| {
            vec![
                ctx::text(s, "id"),
                ctx::when(&s["at_ms"]),
                ctx::text(s, "cid"),
                ctx::s(s, "law").to_string(),
                ctx::s(s, "severity").to_string(),
                ctx::s(s, "sanction").to_string(),
                ctx::s(s, "outcome").to_string(),
                ctx::text(s, "fine"),
                owner_of(s),
            ]
        })
        .collect();
    println!("sanctions");
    ctx::table(
        &[
            "id", "at", "cid", "law", "severity", "sanction", "outcome", "fine", "owner",
        ],
        &rows,
    );
    println!();
    let rows: Vec<Vec<String>> = array(&v["suspensions"])
        .iter()
        .map(|s| {
            vec![
                ctx::text(s, "id"),
                ctx::text(s, "cid"),
                ctx::s(s, "tool").to_string(),
                ctx::s(s, "law").to_string(),
                ctx::when(&s["until_ms"]),
                if s["active"].as_bool().unwrap_or(false) {
                    "yes".to_string()
                } else {
                    "no".to_string()
                },
            ]
        })
        .collect();
    println!("suspensions");
    ctx::table(&["id", "cid", "tool", "law", "until", "active"], &rows);
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { cid, law, limit } => {
            let mut path = format!("/v1/sanctions?limit={limit}");
            if let Some(cid) = cid {
                path.push_str(&format!("&cid={cid}"));
            }
            if let Some(law) = law {
                path.push_str(&format!("&law={}", ctx::seg(&law)));
            }
            let v = ctx.client.get(&path)?;
            ctx.out(&v, register);
        }
        Cmd::Lift { id, reason } => {
            let why = reason.trim();
            if why.is_empty() {
                return Err(format!("say why suspension {id} is lifted: pass --reason"));
            }
            let record = ctx.client.post(
                &format!("/v1/sanctions/{id}/lift"),
                &json!({ "reason": why }),
            )?;
            ctx.out(&record, |r| {
                println!(
                    "sanction {id} lifted: agent {} may use {} again",
                    ctx::text(r, "cid"),
                    ctx::s(r, "tool")
                )
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_column_shows_the_decision_or_the_lift() {
        let decided =
            json!({ "owner_action": { "action": "acknowledge", "at_ms": 1 }, "lifted": null });
        let lifted =
            json!({ "owner_action": null, "lifted": { "at_ms": 2, "by": "owner", "why": "x" } });
        let untouched = json!({ "owner_action": null, "lifted": null });
        assert_eq!(owner_of(&decided), "acknowledge");
        assert_eq!(owner_of(&lifted), "lifted");
        assert_eq!(owner_of(&untouched), "");
    }
}
