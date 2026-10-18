//! What each agent is doing right now: the step its agent view shows (`step`) and the `agent_step`
//! event carries, as `{stage, tool, since_ms}`.
//!
//! The hub learns it from the hook the metrics book an agent's state time from
//! (`metrics::collect::agent_state`: waiting for the model's first token, streaming, running tools,
//! waiting for an approval, idle, asleep) and from each tool call a guest asks for, which names the
//! tool. A step changes many times a minute while an agent works, so it has an event of its own
//! rather than the `agent` one, on which the consoles refetch the roster: `agent_step {cid, step}`,
//! at most once a second per agent. A change inside that second goes out when the second ends, so
//! the last step always reaches the consoles.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use crate::state::App;

/// The least time between two `agent_step` events for one agent, in milliseconds.
const MIN_GAP_MS: u64 = 1_000;
/// How often the held changes are looked at.
const FLUSH: Duration = Duration::from_millis(250);

/// An agent's current step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    /// `thinking` (waiting for the model's first token), `streaming`, `tool`, `approval`, `idle` or
    /// `asleep`.
    pub stage: &'static str,
    /// The tool it runs, or waits for an approval to run, once the guest has named one.
    pub tool: Option<String>,
    /// When the step began, in milliseconds since the Unix epoch.
    pub since_ms: u64,
}

/// One agent's step, and when its event last went out.
struct Entry {
    step: Step,
    /// When the last `agent_step` for the agent went out, if one has.
    published_ms: Option<u64>,
    /// The step changed since then and has not gone out yet.
    held: bool,
}

/// Every agent's step.
#[derive(Default)]
struct Steps {
    entries: Mutex<HashMap<u64, Entry>>,
}

static STEPS: LazyLock<Steps> = LazyLock::new(Steps::default);

impl Steps {
    /// Records that agent `cid` is at `stage` (running `tool`, when one is named) at `now_ms`, and
    /// returns the step when its event is due now. The same step again changes nothing.
    fn set(
        &self,
        cid: u64,
        stage: &'static str,
        tool: Option<String>,
        now_ms: u64,
    ) -> Option<Step> {
        let mut entries = self.lock();
        let entry = entries.entry(cid).or_insert_with(|| Entry {
            step: Step {
                stage,
                tool: None,
                since_ms: now_ms,
            },
            published_ms: None,
            held: true,
        });
        // An approval is for the tool the agent was about to run.
        let tool = tool.or_else(|| {
            (stage == "approval" && entry.step.stage == "tool")
                .then(|| entry.step.tool.clone())
                .flatten()
        });
        if entry.step.stage != stage || entry.step.tool != tool {
            entry.step = Step {
                stage,
                tool,
                since_ms: now_ms,
            };
            entry.held = true;
        }
        due(entry, now_ms)
    }

    /// The held steps whose gap has passed at `now_ms`, as `(cid, step)`: they go out now.
    fn flush(&self, now_ms: u64) -> Vec<(u64, Step)> {
        self.lock()
            .iter_mut()
            .filter_map(|(cid, entry)| due(entry, now_ms).map(|step| (*cid, step)))
            .collect()
    }

    fn get(&self, cid: u64) -> Option<Step> {
        self.lock().get(&cid).map(|entry| entry.step.clone())
    }

    fn forget(&self, cid: u64) {
        self.lock().remove(&cid);
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The entry's step, when it is held and its last event went out at least [`MIN_GAP_MS`] ago (or
/// none has); it is then no longer held.
fn due(entry: &mut Entry, now_ms: u64) -> Option<Step> {
    let gap_passed = entry
        .published_ms
        .is_none_or(|at| now_ms.saturating_sub(at) >= MIN_GAP_MS);
    if !entry.held || !gap_passed {
        return None;
    }
    entry.held = false;
    entry.published_ms = Some(now_ms);
    Some(entry.step.clone())
}

/// The step a metrics state is (`first_token` is `thinking`, `tools` is `tool`), or `None` for a
/// state the step does not show.
fn stage_of(state: &str) -> Option<&'static str> {
    match state {
        "first_token" => Some("thinking"),
        "streaming" => Some("streaming"),
        "tools" => Some("tool"),
        "approval" => Some("approval"),
        "idle" => Some("idle"),
        "asleep" => Some("asleep"),
        _ => None,
    }
}

/// Agent `cid` changed state, as the metrics name it. The `tools` state names no tool yet: the tool
/// requests that follow name each one ([`tool`]).
pub fn state(app: &App, cid: u64, state: &str) {
    if let Some(stage) = stage_of(state) {
        send(app, cid, STEPS.set(cid, stage, None, apz_plaza::now_ms()));
    }
}

/// Agent `cid` asked to run `name`.
pub fn tool(app: &App, cid: u64, name: &str) {
    let step = STEPS.set(cid, "tool", Some(name.to_string()), apz_plaza::now_ms());
    send(app, cid, step);
}

/// Agent `cid`'s current step, if it has reported one since the hub started.
pub fn of(cid: u64) -> Option<Step> {
    STEPS.get(cid)
}

/// Drops a removed agent's step.
pub fn forget(cid: u64) {
    STEPS.forget(cid);
}

/// Starts the thread that sends each held step once its agent's gap has passed.
pub fn spawn(app: Arc<App>) {
    let spawned = std::thread::Builder::new()
        .name("apid-steps".into())
        .spawn(move || {
            loop {
                std::thread::sleep(FLUSH);
                for (cid, step) in STEPS.flush(apz_plaza::now_ms()) {
                    send(&app, cid, Some(step));
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("steps: cannot start: {err}");
    }
}

fn send(app: &App, cid: u64, step: Option<Step>) {
    if let Some(step) = step {
        app.bus
            .publish("agent_step", &json!({ "cid": cid, "step": step }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(stage: &'static str, tool: Option<&str>, since_ms: u64) -> Step {
        Step {
            stage,
            tool: tool.map(str::to_string),
            since_ms,
        }
    }

    #[test]
    fn a_step_goes_out_at_once_then_at_most_once_a_second() {
        let steps = Steps::default();
        assert_eq!(
            steps.set(7, "thinking", None, 10_000),
            Some(step("thinking", None, 10_000)),
            "the first step goes out at once"
        );
        assert_eq!(
            steps.set(7, "streaming", None, 10_300),
            None,
            "a change inside the second is held"
        );
        assert_eq!(steps.set(7, "tool", Some("bash".into()), 10_600), None);
        assert_eq!(
            steps.get(7),
            Some(step("tool", Some("bash"), 10_600)),
            "the view sees the newest step at once"
        );
        assert!(steps.flush(10_900).is_empty(), "the second has not passed");
        assert_eq!(
            steps.flush(11_000),
            vec![(7, step("tool", Some("bash"), 10_600))],
            "the newest held step goes out when the second ends"
        );
        assert!(steps.flush(12_500).is_empty(), "nothing is held");
        assert_eq!(
            steps.set(7, "idle", None, 12_500),
            Some(step("idle", None, 12_500)),
            "after a quiet second a change goes out at once"
        );
    }

    #[test]
    fn the_same_step_again_changes_nothing() {
        let steps = Steps::default();
        steps.set(7, "idle", None, 1_000);
        assert_eq!(steps.set(7, "idle", None, 5_000), None);
        assert_eq!(steps.get(7), Some(step("idle", None, 1_000)));
        assert!(steps.flush(9_000).is_empty());
    }

    #[test]
    fn an_approval_is_for_the_tool_the_agent_was_about_to_run() {
        let steps = Steps::default();
        steps.set(7, "tool", Some("plaza_pay".into()), 1_000);
        steps.set(7, "approval", None, 3_000);
        assert_eq!(
            steps.get(7),
            Some(step("approval", Some("plaza_pay"), 3_000))
        );
        // A new turn's tools name no tool until a request does.
        steps.set(7, "thinking", None, 9_000);
        steps.set(7, "tool", None, 12_000);
        assert_eq!(steps.get(7), Some(step("tool", None, 12_000)));
    }

    #[test]
    fn agents_are_held_apart_and_a_removed_one_is_forgotten() {
        let steps = Steps::default();
        steps.set(7, "thinking", None, 1_000);
        assert!(
            steps.set(8, "thinking", None, 1_100).is_some(),
            "another agent's gap is its own"
        );
        steps.forget(7);
        assert_eq!(steps.get(7), None);
        assert!(steps.get(8).is_some());
    }

    #[test]
    fn the_metrics_states_map_to_steps() {
        assert_eq!(stage_of("first_token"), Some("thinking"));
        assert_eq!(stage_of("streaming"), Some("streaming"));
        assert_eq!(stage_of("tools"), Some("tool"));
        assert_eq!(stage_of("approval"), Some("approval"));
        assert_eq!(stage_of("idle"), Some("idle"));
        assert_eq!(stage_of("asleep"), Some("asleep"));
        assert_eq!(stage_of("something else"), None);
    }
}
