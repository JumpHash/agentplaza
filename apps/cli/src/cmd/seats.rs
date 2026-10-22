//! `agentplaza seats <task>`: a task's seats, each filled by one session at a time: its state, the step it belongs to,
//! its model, the session filling it now, its attempts and the round of its recipe's loop.

use crate::ctx::{self, Ctx};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// The task's id.
    pub task: String,
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let seats = ctx
        .client
        .get(&format!("/v1/tasks/{}/seats", ctx::seg(&args.task)))?;
    ctx.out(&seats, |seats| {
        let rows: Vec<Vec<String>> = seats
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|s| {
                vec![
                    ctx::s(s, "name").to_string(),
                    ctx::s(s, "state").to_string(),
                    ctx::s(s, "role").to_string(),
                    ctx::s(s, "step").to_string(),
                    or_dash(s, "last_model"),
                    or_dash(s, "session"),
                    ctx::text(s, "attempts"),
                    ctx::text(s, "round"),
                ]
            })
            .collect();
        ctx::table(
            &[
                "seat", "state", "role", "step", "model", "session", "attempts", "round",
            ],
            &rows,
        );
    });
    Ok(())
}

/// A text field, or `-` when it is missing or empty.
fn or_dash(v: &serde_json::Value, key: &str) -> String {
    match ctx::text(v, key) {
        text if text.is_empty() => "-".to_string(),
        text => text,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_seat_without_a_session_or_model_shows_a_dash() {
        let seat = json!({ "name": "builder", "last_model": null, "session": null, "attempts": 0 });
        assert_eq!(or_dash(&seat, "session"), "-");
        assert_eq!(or_dash(&seat, "last_model"), "-");
        assert_eq!(or_dash(&json!({ "session": "s12" }), "session"), "s12");
    }
}
