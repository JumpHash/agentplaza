//! The general adapter's scripting surface: a [`Bridge`] runs `code` and returns what it printed.
//!
//! Only the `cli` bridge is built: a command line run in the work folder through `apz_ops` runs, synchronous
//! with a timeout. The app bridges (AppleScript, JXA, PowerShell, COM, D-Bus, the apps' own scripting) are
//! refused as not built yet, so an agent learns that instead of getting silence.

use std::thread;
use std::time::{Duration, Instant};

use apz_ops::{Env, Root};
use apz_station::Bridge;
use serde_json::{Value, json};

use crate::daemon::Shared;

/// How often a synchronous script is polled for its end.
const POLL: Duration = Duration::from_millis(50);
/// How long past its own timeout a script may run before it is stopped here.
const GRACE: Duration = Duration::from_secs(5);

/// Runs `code` through `bridge`. The result is the script operation's: `{"exit_code", "output", "timed_out"}`
/// for the `cli` bridge.
pub fn run(
    shared: &Shared,
    bridge: Bridge,
    app: Option<&str>,
    code: &str,
    timeout_s: u32,
) -> Result<Value, String> {
    match bridge {
        Bridge::Cli => {
            if app.is_some() {
                return Err("the cli bridge takes no app".to_string());
            }
            let root = shared.root();
            cli(shared, &root, code, timeout_s)
        }
        other => Err(format!(
            "the {} bridge is not built in this version",
            bridge_name(other)
        )),
    }
}

/// Runs a command line in `root` and waits for it, up to `timeout_s` (the runs' own limit applies too).
pub fn cli(shared: &Shared, root: &Root, code: &str, timeout_s: u32) -> Result<Value, String> {
    let env: &Env = &shared.env;
    let started = shared
        .runs
        .start(root, env, code, Some(u64::from(timeout_s)))
        .map_err(|e| e.to_string())?;
    let run = started.run;
    shared
        .run_ids
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(run.clone());

    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_s)) + GRACE;
    let mut output = String::new();
    let mut offset = 0u64;
    let (exit_code, timed_out) = loop {
        let poll = shared
            .runs
            .poll(&run, Some(offset))
            .map_err(|e| e.to_string())?;
        output.push_str(&poll.output);
        offset = poll.next;
        if poll.done {
            break (poll.exit_code, poll.timed_out);
        }
        if Instant::now() >= deadline {
            let _ = shared.runs.stop(&run);
            break (None, true);
        }
        thread::sleep(POLL);
    };
    shared
        .run_ids
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run);
    Ok(json!({
        "exit_code": exit_code,
        "output": output,
        "timed_out": timed_out,
    }))
}

/// The wire name of a bridge, as the protocol spells it.
pub fn bridge_name(bridge: Bridge) -> String {
    serde_json::to_value(bridge)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridges_have_their_wire_names() {
        assert_eq!(bridge_name(Bridge::Cli), "cli");
        assert_eq!(bridge_name(Bridge::AppScripting), "app_scripting");
        assert_eq!(bridge_name(Bridge::PowerShell), "powershell");
    }
}
