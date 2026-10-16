//! Agents' memory follows their work. Every 5 s apid reads each agent VM's
//! balloon state from noded and resizes its usable memory: more while the
//! agent works or runs short of available memory, less once it has been idle
//! for a minute. The policy is the pure function [`target`]; [`spawn`] runs it.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::agents::Status;
use crate::state::App;

/// The smallest usable memory a VM is given, in MiB.
const FLOOR_MIB: u32 = 32;
/// A working agent has at least this much usable memory, in MiB.
const WORKING_MIB: u32 = 192;
/// Below this much available memory the VM grows, in MiB.
const LOW_AVAILABLE_MIB: u32 = 24;
/// Growth per step, in MiB.
const GROW_MIB: u32 = 64;
/// Shrink per pass while idle, in MiB.
const SHRINK_MIB: u32 = 32;
/// Headroom kept above what the guest uses, in MiB.
const HEADROOM_MIB: u32 = 16;
/// How long an agent must be idle before its memory shrinks, in ms.
const IDLE_AFTER_MS: u64 = 60_000;
/// The smallest change worth a resize, in MiB.
const MIN_CHANGE_MIB: u32 = 8;
/// Time between passes.
const PASS_EVERY: Duration = Duration::from_secs(5);

/// One agent VM's memory as the policy sees it. Sizes are MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub working: bool,
    /// When (ms) the agent was last seen working.
    pub last_busy_ms: u64,
    pub max_mib: u32,
    pub usable_mib: u32,
    pub available_mib: u32,
}

/// The usable memory this VM should have now, or `None` when the change is
/// under 8 MiB. The result is clamped to [32, max].
pub fn target(state: &Sample, now_ms: u64) -> Option<u32> {
    let used = state.max_mib.saturating_sub(state.available_mib);
    let idle_long = !state.working && now_ms.saturating_sub(state.last_busy_ms) > IDLE_AFTER_MS;
    let mut wanted = state.usable_mib;
    if state.working {
        wanted = wanted.max(WORKING_MIB);
    }
    if state.available_mib < LOW_AVAILABLE_MIB {
        wanted += GROW_MIB;
    } else if idle_long {
        // Shrink a step, but never below what the guest uses plus headroom,
        // and never upward.
        wanted = wanted
            .saturating_sub(SHRINK_MIB)
            .max(used + HEADROOM_MIB)
            .min(state.usable_mib);
    }
    let clamped = wanted.clamp(FLOOR_MIB.min(state.max_mib), state.max_mib);
    (clamped.abs_diff(state.usable_mib) >= MIN_CHANGE_MIB).then_some(clamped)
}

/// Starts the autoscaler thread: one pass every 5 s.
pub fn spawn(app: Arc<App>) {
    std::thread::spawn(move || {
        let mut last_busy: BTreeMap<u64, u64> = BTreeMap::new();
        loop {
            pass(&app, &mut last_busy);
            std::thread::sleep(PASS_EVERY);
        }
    });
}

/// One pass over the roster. Each run is read once per pass; noded errors
/// skip the affected agents for this pass only.
fn pass(app: &App, last_busy: &mut BTreeMap<u64, u64>) {
    if !app.settings.get().memory_autoscale {
        return;
    }
    let now = apz_plaza::now_ms();
    let mut runs: HashMap<String, Option<Value>> = HashMap::new();
    for agent in app.agents.list() {
        if agent.status == Status::Offline {
            last_busy.remove(&agent.cid);
            continue;
        }
        let working = agent.status == Status::Working;
        let slot = last_busy.entry(agent.cid).or_insert(now);
        if working {
            *slot = now;
        }
        let last = *slot;
        let (Some(run), Some(_)) = (agent.run.clone(), agent.vm) else {
            continue;
        };
        let cached = runs
            .entry(run.clone())
            .or_insert_with(|| read_run(app, &run));
        let Some(view) = cached.as_ref() else {
            continue;
        };
        // Agents are placed by guest CID (see agents::reconcile_once).
        let Some(vm) = view["vms"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|vm| vm["guest_cid"].as_u64() == Some(agent.cid))
        else {
            continue;
        };
        let memory = &vm["memory"];
        if memory.is_null() {
            continue;
        }
        let (Some(index), Some(max), Some(usable), Some(available)) = (
            vm["id"].as_u64(),
            mib(&memory["max_mib"]),
            mib(&memory["usable_mib"]),
            mib(&memory["available_mib"]),
        ) else {
            continue;
        };
        let sample = Sample {
            working,
            last_busy_ms: last,
            max_mib: max,
            usable_mib: usable,
            available_mib: available,
        };
        let Some(next) = target(&sample, now) else {
            continue;
        };
        let reason = if working { "working" } else { "idle" };
        match app.noded.post(
            &format!("/v1/runs/{run}/vms/{index}/memory"),
            &json!({"usable_mib": next}),
        ) {
            Ok((status, _)) if status < 300 => {
                eprintln!(
                    "apid: agent {} VM {run}/{index} memory {usable} -> {next} MiB ({reason})",
                    agent.cid
                );
            }
            Ok((status, _)) => {
                eprintln!(
                    "apid: agent {} VM {run}/{index} memory resize refused: noded {status}",
                    agent.cid
                );
            }
            Err(err) => eprintln!(
                "apid: agent {} VM {run}/{index} memory resize failed: {err}",
                agent.cid
            ),
        }
    }
}

/// `GET /v1/runs/{run}` from noded, or `None` when it is not available.
fn read_run(app: &App, run: &str) -> Option<Value> {
    match app.noded.get(&format!("/v1/runs/{run}")) {
        Ok((200, body)) => Some(body),
        _ => None,
    }
}

fn mib(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn sample(working: bool, last_busy_ms: u64, max: u32, usable: u32, available: u32) -> Sample {
        Sample {
            working,
            last_busy_ms,
            max_mib: max,
            usable_mib: usable,
            available_mib: available,
        }
    }

    #[test]
    fn target_steps_grow_and_shrink_within_bounds() {
        // Working agent gets at least 192 MiB.
        // Low available grows by a step whatever the work.
        // Idle shrinks gradually after a minute.
        // Idle never goes below used plus headroom (used = 512 - 312 = 200, so the floor is 216).
        // Target is clamped to the floor and max.
        for (case, state, want) in [
            (
                "working agent with 128 usable gets 192",
                sample(true, NOW, 512, 128, 100),
                Some(192),
            ),
            (
                "working agent with 256 usable stays",
                sample(true, NOW, 512, 256, 100),
                None,
            ),
            (
                "working agent with 190 usable stays",
                sample(true, NOW, 512, 190, 100),
                None,
            ),
            (
                "low available grows a step while working",
                sample(true, NOW, 512, 256, 10),
                Some(320),
            ),
            (
                "low available grows a step while idle",
                sample(false, NOW, 512, 256, 10),
                Some(320),
            ),
            (
                "idle shrinks to 224 first",
                sample(false, 0, 512, 256, 400),
                Some(224),
            ),
            (
                "idle shrinks to 192 next",
                sample(false, 0, 512, 224, 400),
                Some(192),
            ),
            (
                "idle under a minute stays",
                sample(false, NOW - 59_000, 512, 256, 400),
                None,
            ),
            (
                "idle target is used plus headroom",
                sample(false, 0, 512, 300, 312),
                Some(268),
            ),
            (
                "idle target at the floor",
                sample(false, 0, 512, 230, 312),
                Some(216),
            ),
            (
                "idle below the floor stays",
                sample(false, 0, 512, 220, 312),
                None,
            ),
            ("growth stops at max", sample(true, NOW, 256, 250, 0), None),
            (
                "growth reaches max",
                sample(true, NOW, 256, 200, 0),
                Some(256),
            ),
            (
                "shrinking stops at 32 MiB",
                sample(false, 0, 256, 40, 256),
                Some(32),
            ),
            (
                "shrinking at 32 MiB stays",
                sample(false, 0, 256, 32, 256),
                None,
            ),
        ] {
            assert_eq!(target(&state, NOW), want, "{case}");
        }
    }
}
