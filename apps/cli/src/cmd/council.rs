//! `agentplaza council`: The council: motions, their votes, and the owner's decision on each.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The motions, newest first, with their tallies.
    List {
        /// Only the motions in this state (for example pending).
        #[arg(long)]
        state: Option<String>,
    },
    /// One motion: its text, its tally and every vote.
    Show {
        /// The motion id.
        id: String,
    },
    /// Decide a motion as the owner.
    Decide {
        /// The motion id.
        id: String,
        /// approve or reject
        #[arg(value_parser = ["approve", "reject"])]
        decision: String,
        /// Why, recorded with the decision.
        #[arg(long, default_value = "")]
        reason: String,
    },
}

/// The motions of the `/v1/motions` answer.
fn motions(v: &Value) -> &[Value] {
    v.get("motions").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// A motion's tally as one line.
fn tally(m: &Value) -> String {
    let field = |key: &str| m.get("tally").map(|t| ctx::text(t, key)).unwrap_or_default();
    format!("yes {} no {} abstain {} ({} of {} seats voted)", field("yes"), field("no"), field("abstain"), field("voted"), field("seats"))
}

/// A motion's quorum: the rule in words, from the motion's `quorum` view.
fn quorum(m: &Value) -> String {
    m.get("quorum").map(|q| ctx::text(q, "rule")).unwrap_or_default()
}

/// One vote as a `show` row: the agent that cast it, its seat, the word and the reason.
fn vote_row(x: &Value) -> Vec<String> {
    vec![ctx::text(x, "agent_cid"), ctx::text(x, "seat"), ctx::text(x, "vote"), ctx::text(x, "reason")]
}

/// The `POST /v1/motions/{id}/decide` body: `approve` as the hub reads it, and the note, or null
/// when the reason is blank (the hub refuses an empty note, but takes an absent one).
fn decide_body(decision: &str, reason: &str) -> Value {
    let note = reason.trim();
    json!({ "approve": decision == "approve", "note": if note.is_empty() { Value::Null } else { json!(note) } })
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { state } => {
            let mut v = ctx.client.get("/v1/motions")?;
            if let Some(state) = state {
                if let Some(list) = v.get_mut("motions").and_then(Value::as_array_mut) {
                    list.retain(|m| ctx::s(m, "state") == state.as_str());
                }
            }
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = motions(v)
                    .iter()
                    .map(|m| {
                        vec![
                            ctx::s(m, "id").to_string(),
                            ctx::s(m, "kind").to_string(),
                            ctx::s(m, "state").to_string(),
                            tally(m),
                            ctx::text(m, "proposer_cid"),
                            ctx::s(m, "text").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "kind", "state", "tally", "proposer", "text"], &rows);
            });
        }
        Cmd::Show { id } => {
            let v = ctx.client.get("/v1/motions")?;
            let motion = motions(&v)
                .iter()
                .find(|m| ctx::s(m, "id") == id.as_str())
                .cloned()
                .ok_or_else(|| format!("no motion {id}"))?;
            ctx.out(&motion, |m| {
                ctx::fields(&[
                    ("id", ctx::text(m, "id")),
                    ("kind", ctx::text(m, "kind")),
                    ("state", ctx::text(m, "state")),
                    ("proposer", ctx::text(m, "proposer_cid")),
                    ("tally", tally(m)),
                    ("quorum", quorum(m)),
                    ("text", ctx::text(m, "text")),
                ]);
                println!();
                let votes: Vec<Vec<String>> = m.get("votes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]).iter().map(vote_row).collect();
                ctx::table(&["agent", "seat", "vote", "reason"], &votes);
            });
        }
        Cmd::Decide { id, decision, reason } => {
            let v = ctx.client.post(&format!("/v1/motions/{}/decide", ctx::seg(&id)), &decide_body(&decision, &reason))?;
            let past = if decision == "approve" { "approved" } else { "rejected" };
            ctx.out(&v, |_| println!("{past} motion {id}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_body_serialises_approve_and_note_as_the_hub_reads_them() {
        assert_eq!(serde_json::to_string(&decide_body("approve", "fine")).unwrap(), r#"{"approve":true,"note":"fine"}"#);
        assert_eq!(serde_json::to_string(&decide_body("reject", "  no  ")).unwrap(), r#"{"approve":false,"note":"no"}"#);
        assert_eq!(serde_json::to_string(&decide_body("reject", "")).unwrap(), r#"{"approve":false,"note":null}"#);
    }

    #[test]
    fn a_vote_row_reads_its_seat_and_agent() {
        let row = vote_row(&json!({ "seat": "architect", "agent_cid": 42, "vote": "yes", "reason": "sound" }));
        assert_eq!(row, vec!["42", "architect", "yes", "sound"]);
    }

    #[test]
    fn a_motion_shows_its_quorum_rule() {
        let m = json!({ "quorum": { "rule": "3 of the 4 filled seats", "seats": 4, "filled": 4, "required": 3, "can_decide": true } });
        assert_eq!(quorum(&m), "3 of the 4 filled seats");
        assert_eq!(quorum(&json!({})), "");
    }
}
