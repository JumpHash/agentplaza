//! `agents cancel`, `retire` and `wipe`: an agent's running work, and its place on the plaza.

use serde_json::{json, Value};

use crate::ctx::{confirm, Ctx};

/// What a wipe clears on top of the workspace, which is always reset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WipeWhat {
    pub memory: bool,
    pub transcript: bool,
    pub identity: bool,
}

/// Stops the agent's running work. The agent stays on the roster.
pub fn cancel(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx.client.post(&format!("/v1/agents/{cid}/cancel"), &json!({}))?;
    ctx.out(&v, |v| {
        if v.get("cancelled").and_then(Value::as_bool).unwrap_or(false) {
            println!("cancelled the running work of agent {cid}");
        } else {
            println!("agent {cid} had no work running");
        }
    });
    Ok(())
}

/// Retires an agent for good: its VM stops and it leaves the roster. Asks to confirm unless `yes`.
pub fn retire(ctx: &Ctx, cid: u64, yes: bool) -> Result<(), String> {
    if !confirm(yes, &format!("Retire agent {cid}? Its VM stops for good and it leaves the roster."))? {
        return super::cancelled();
    }
    let v = ctx.client.post(&format!("/v1/agents/{cid}/retire"), &json!({}))?;
    ctx.out(&v, |_| println!("retired agent {cid}"));
    Ok(())
}

/// The wipe request body. At least one of memory, transcript or identity is needed: a wipe that
/// only resets the workspace is refused, so the owner names what else to clear.
pub fn wipe_body(what: WipeWhat) -> Result<Value, String> {
    if !(what.memory || what.transcript || what.identity) {
        return Err("a wipe needs at least one of --memory, --transcript, --identity (the workspace is always reset)".into());
    }
    Ok(json!({ "memory": what.memory, "transcript": what.transcript, "identity": what.identity }))
}

/// Wipes an agent's workspace and what `what` names. Asks to confirm unless `yes`.
pub fn wipe(ctx: &Ctx, cid: u64, what: WipeWhat, yes: bool) -> Result<(), String> {
    let body = wipe_body(what)?;
    let mut cleared = vec!["workspace"];
    if what.memory {
        cleared.push("memory");
    }
    if what.transcript {
        cleared.push("transcript");
    }
    if what.identity {
        cleared.push("identity (name, contacts and reputation)");
    }
    if !confirm(yes, &format!("Wipe agent {cid}: clear its {}?", cleared.join(", ")))? {
        return super::cancelled();
    }
    let v = ctx.client.post(&format!("/v1/agents/{cid}/wipe"), &body)?;
    ctx.out(&v, |v| {
        let wiped: Vec<&str> = v["wiped"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        println!("wiped {} of agent {cid}", wiped.join(", "));
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wipe_needs_at_least_one_flag() {
        assert!(wipe_body(WipeWhat::default()).is_err());
    }

    #[test]
    fn a_wipe_body_names_each_flag() {
        let body = wipe_body(WipeWhat { memory: true, transcript: false, identity: true }).unwrap();
        assert_eq!(body, json!({ "memory": true, "transcript": false, "identity": true }));
        let body = wipe_body(WipeWhat { transcript: true, ..WipeWhat::default() }).unwrap();
        assert_eq!(body["transcript"], true);
        assert_eq!(body["memory"], false);
    }
}
