//! `agents harness` and `agents loops`: an agent's limits and running work, and the loops
//! (recurring prompts) that run on agents. The web's harness card sends the same bodies.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, text_arg};

/// Most characters a steer note may have (the hub's limit).
const STEER_MAX_CHARS: usize = 2000;
/// Most turns one extension may add (the hub's limit).
const EXTEND_MAX: u32 = 200;
/// Bytes in a KB: the limit `compress` takes in bytes is set in KB here, as the web does.
const KB: f64 = 1024.0;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Set limits. Only the flags given change; the others keep their value.
    Set {
        /// Turns per work item (1 to 500).
        #[arg(long)]
        turns: Option<u32>,
        /// Compress the conversation once it passes this many KB (0 never).
        #[arg(long)]
        compress_kb: Option<f64>,
        /// Recent turns kept when the conversation is compressed (1 to 50).
        #[arg(long)]
        keep_recent: Option<u32>,
    },
    /// Compress the running work's conversation now.
    Compress,
    /// Give the running work more turns.
    Extend {
        /// How many more turns (1 to 200).
        #[arg(long, default_value_t = 20)]
        turns: u32,
    },
    /// Send a note into the running work. "-" reads it from stdin.
    Steer {
        /// The note.
        text: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum LoopAction {
    /// Start a loop: the prompt runs on the agent every so many minutes.
    Start {
        /// The agent's cid.
        cid: u64,
        /// What the agent does on each run. "-" reads it from stdin.
        #[arg(long)]
        prompt: String,
        /// Minutes between runs (at least 1).
        #[arg(long)]
        every: u64,
        /// How many runs; 0 runs until the agent's answer says LOOP DONE.
        #[arg(long, default_value_t = 0)]
        max_runs: u32,
    },
    /// Stop a loop by its id (see `agents loops`).
    Stop {
        /// The loop's id.
        id: String,
    },
}

pub fn harness(ctx: &Ctx, cid: u64, action: Option<Action>) -> Result<(), String> {
    match action {
        None => show(ctx, cid),
        Some(Action::Set {
            turns,
            compress_kb,
            keep_recent,
        }) => {
            let body = limits_body(turns, compress_kb, keep_recent)?;
            control(ctx, cid, &body)
        }
        Some(Action::Compress) => control(ctx, cid, &json!({ "action": "compress" })),
        Some(Action::Extend { turns }) => control(ctx, cid, &extend_body(turns)?),
        Some(Action::Steer { text }) => control(ctx, cid, &steer_body(&text_arg(&text)?)?),
    }
}

/// An agent's limits: a blank one is the plaza's default.
fn show(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let limits = ctx.client.get(&format!("/v1/agents/{cid}/limits"))?;
    ctx.out(&limits, |l| {
        ctx::fields(&[
            ("turns per work item", or_default(&l["max_turns"])),
            ("compress at", kb_text(&l["compress_at"])),
            ("keep recent turns", or_default(&l["keep_recent"])),
        ]);
    });
    Ok(())
}

/// Posts a control body; the hub's answer is a line of text (or the saved limits).
fn control(ctx: &Ctx, cid: u64, body: &Value) -> Result<(), String> {
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/control"), body)?;
    let action = body["action"].as_str().unwrap_or("");
    ctx.out(&v, |v| {
        if action == "set_limits" {
            println!("saved the limits of agent {cid}");
        } else {
            println!("{}", ctx::text(v, "result"));
        }
    });
    Ok(())
}

/// The `set_limits` body: only the limits given. Compress is given in KB and sent in bytes.
pub fn limits_body(
    turns: Option<u32>,
    compress_kb: Option<f64>,
    keep_recent: Option<u32>,
) -> Result<Value, String> {
    if turns.is_none() && compress_kb.is_none() && keep_recent.is_none() {
        return Err("set needs at least one of --turns, --compress-kb, --keep-recent".into());
    }
    let mut body = json!({ "action": "set_limits" });
    if let Some(turns) = turns {
        body["max_turns"] = json!(turns);
    }
    if let Some(kb) = compress_kb {
        body["compress_at"] = json!(kb_to_bytes(kb)?);
    }
    if let Some(keep) = keep_recent {
        body["keep_recent"] = json!(keep);
    }
    Ok(body)
}

/// KB to bytes, rounded to the whole byte. A negative or non-finite size is refused.
pub fn kb_to_bytes(kb: f64) -> Result<u64, String> {
    if !kb.is_finite() || kb < 0.0 {
        return Err("--compress-kb is a number of 0 or more".into());
    }
    Ok((kb * KB).round() as u64)
}

/// The `extend_turns` body.
pub fn extend_body(turns: u32) -> Result<Value, String> {
    if !(1..=EXTEND_MAX).contains(&turns) {
        return Err(format!("--turns is 1 to {EXTEND_MAX}"));
    }
    Ok(json!({ "action": "extend_turns", "turns": turns }))
}

/// The `steer` body; the note is already trimmed and non-empty (see `text_arg`).
pub fn steer_body(text: &str) -> Result<Value, String> {
    if text.chars().count() > STEER_MAX_CHARS {
        return Err(format!(
            "the steer note is over {STEER_MAX_CHARS} characters"
        ));
    }
    Ok(json!({ "action": "steer", "text": text }))
}

/// `agents loops`: every loop, or only the agent `cid`'s; then `start` and `stop`.
pub fn loops(ctx: &Ctx, cid: Option<u64>, action: Option<LoopAction>) -> Result<(), String> {
    match action {
        None => list_loops(ctx, cid),
        Some(LoopAction::Start {
            cid,
            prompt,
            every,
            max_runs,
        }) => {
            let body = loop_body(cid, &text_arg(&prompt)?, every, max_runs)?;
            let v = ctx.client.post("/v1/loops", &body)?;
            ctx.out(&v, |l| {
                println!("started loop {} for agent {cid}", ctx::text(l, "id"))
            });
            Ok(())
        }
        Some(LoopAction::Stop { id }) => {
            let v = ctx.client.delete(&format!("/v1/loops/{}", ctx::seg(&id)))?;
            ctx.out(&v, |_| println!("stopped loop {id}"));
            Ok(())
        }
    }
}

/// The `POST /v1/loops` body.
pub fn loop_body(
    cid: u64,
    prompt: &str,
    every_minutes: u64,
    max_runs: u32,
) -> Result<Value, String> {
    if every_minutes < 1 {
        return Err("--every is at least 1 minute".into());
    }
    Ok(
        json!({ "cid": cid, "prompt": prompt, "every_minutes": every_minutes, "max_runs": max_runs }),
    )
}

fn list_loops(ctx: &Ctx, cid: Option<u64>) -> Result<(), String> {
    let all = ctx.client.get("/v1/loops")?;
    let shown: Vec<Value> = super::items(&all)
        .iter()
        .filter(|l| cid.is_none_or(|c| l["cid"].as_u64() == Some(c)))
        .cloned()
        .collect();
    ctx.out(&Value::Array(shown.clone()), |_| {
        let rows: Vec<Vec<String>> = shown
            .iter()
            .map(|l| {
                let max = l["max_runs"].as_u64().unwrap_or(0);
                let max = if max == 0 {
                    "∞".to_string()
                } else {
                    max.to_string()
                };
                vec![
                    ctx::text(l, "id"),
                    ctx::text(l, "cid"),
                    ctx::s(l, "state").to_string(),
                    format!("{} min", l["every_secs"].as_u64().unwrap_or(0) / 60),
                    format!("{}/{}", ctx::text(l, "runs"), max),
                    ctx::text(l, "prompt"),
                ]
            })
            .collect();
        ctx::table(&["id", "cid", "state", "every", "runs", "prompt"], &rows);
    });
    Ok(())
}

/// A limit's value, or "plaza default" when it is blank.
fn or_default(v: &Value) -> String {
    if v.is_null() {
        "plaza default".to_string()
    } else {
        v.to_string()
    }
}

/// A compress size in bytes as KB; 0 is "never"; blank is the plaza default.
fn kb_text(bytes: &Value) -> String {
    match bytes.as_u64() {
        Some(0) => "never".to_string(),
        Some(n) => format!("{} KB", n as f64 / KB),
        None => or_default(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kilobytes_become_whole_bytes() {
        assert_eq!(kb_to_bytes(20.0).unwrap(), 20_480);
        assert_eq!(kb_to_bytes(0.5).unwrap(), 512);
        assert_eq!(kb_to_bytes(19.5).unwrap(), 19_968);
        assert_eq!(kb_to_bytes(0.0).unwrap(), 0);
    }

    #[test]
    fn a_negative_or_non_finite_size_is_refused() {
        assert!(kb_to_bytes(-1.0).is_err());
        assert!(kb_to_bytes(f64::NAN).is_err());
        assert!(kb_to_bytes(f64::INFINITY).is_err());
    }

    #[test]
    fn set_sends_only_the_limits_given_with_compress_in_bytes() {
        let body = limits_body(Some(40), Some(20.0), None).unwrap();
        assert_eq!(
            body,
            json!({ "action": "set_limits", "max_turns": 40, "compress_at": 20_480 })
        );
        let body = limits_body(None, None, Some(3)).unwrap();
        assert_eq!(body, json!({ "action": "set_limits", "keep_recent": 3 }));
    }

    #[test]
    fn set_needs_at_least_one_limit() {
        assert!(limits_body(None, None, None).is_err());
    }

    #[test]
    fn extend_takes_one_to_two_hundred_turns() {
        assert_eq!(
            extend_body(20).unwrap(),
            json!({ "action": "extend_turns", "turns": 20 })
        );
        assert!(extend_body(0).is_err());
        assert!(extend_body(201).is_err());
    }

    #[test]
    fn a_steer_note_is_at_most_two_thousand_characters() {
        assert_eq!(
            steer_body("slow down").unwrap(),
            json!({ "action": "steer", "text": "slow down" })
        );
        assert!(steer_body(&"x".repeat(2000)).is_ok());
        assert!(steer_body(&"x".repeat(2001)).is_err());
    }

    #[test]
    fn a_loop_runs_at_least_every_minute() {
        let body = loop_body(7, "check the queue", 30, 0).unwrap();
        assert_eq!(
            body,
            json!({ "cid": 7, "prompt": "check the queue", "every_minutes": 30, "max_runs": 0 })
        );
        assert!(loop_body(7, "check", 0, 0).is_err());
    }

    #[test]
    fn limits_read_as_the_plaza_default_when_blank() {
        assert_eq!(or_default(&Value::Null), "plaza default");
        assert_eq!(kb_text(&Value::Null), "plaza default");
        assert_eq!(kb_text(&json!(0)), "never");
        assert_eq!(kb_text(&json!(20_480)), "20 KB");
        assert_eq!(kb_text(&json!(19_968)), "19.5 KB");
    }
}
