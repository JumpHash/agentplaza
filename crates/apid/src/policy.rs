//! The policy snapshot (PLAN 13.2, lever 1): the tools an agent's VM runs without asking the hub.
//!
//! The hub decides every call a model makes, so a turn of several calls waits on a round trip for each. The calls
//! whose decision cannot depend on the call's input or on the owner (a guest tool the agent's grant covers, which the
//! laws permit for it and read no input of, in an autonomous project) are settled in the VM instead. The hub signs
//! the list of tools the VM may run (a [`PolicySnapshot`]) and pushes it at hello and whenever the list changes; the
//! VM reports each call it ran under the snapshot's id, and the hub records the call with that policy. A report the
//! snapshot does not cover is a violation: it is recorded, and the call stands. Every other call goes to the hub in
//! the turn's batch, which decides it as `tool_request` does (`internal::tool_batch`).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_proto::wire::{HostMessage, PolicySnapshot};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::agents::{Agent, Status, profile};
use crate::approvals::Mode;
use crate::laws::Laws;
use crate::state::App;
use crate::tools::{Runs, runs};

/// The snapshots kept for each agent. A report names one of them; one older than these is a violation.
const KEPT: usize = 64;

/// The snapshots the hub issued to each agent (by CID), oldest first.
static ISSUED: Mutex<BTreeMap<u64, Vec<PolicySnapshot>>> = Mutex::new(BTreeMap::new());
/// The allow list last sent to each agent (by CID): a push goes out only when it changes.
static SENT: Mutex<BTreeMap<u64, Vec<String>>> = Mutex::new(BTreeMap::new());
/// Serialises the issuing and the pushes, so the last snapshot sent carries the newest state.
static PUSHING: Mutex<()> = Mutex::new(());

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The tools an agent of `role` (CID `cid`, granted `granted`) runs without asking the hub: the guest's own tools its
/// grant covers, which the laws permit for this agent and no law reads the input of. Nothing while the agent's mode
/// is not autonomous (ask and read-only decide each call) or while it is held (STOP ALL, or its project paused or
/// over).
pub(crate) fn allowed(
    laws: &Laws,
    cid: u64,
    role: &str,
    granted: &[String],
    mode: Mode,
    held: bool,
) -> Vec<String> {
    if held || mode != Mode::Autonomous {
        return Vec::new();
    }
    let mut tools: Vec<String> = granted
        .iter()
        .filter(|tool| local_candidate(tool))
        .filter(|tool| !laws.tests_input(tool))
        .filter(|tool| laws.permits_for(cid, role, tool))
        .cloned()
        .collect();
    tools.sort();
    tools.dedup();
    tools
}

/// Whether the hub's decision on a call of `tool` is one the VM may make alone: a guest tool the hub only allows.
/// Not a host tool, a library program, an engine or a packaged tool (the hub runs or settles those itself), and not
/// a tool a VM checkpoint comes before.
fn local_candidate(tool: &str) -> bool {
    runs(tool) == Some(Runs::Guest)
        && !tool.starts_with(crate::library::PREFIX)
        && !tool.starts_with("engine.")
        && !matches!(tool, "code_engine" | "tool_package")
        && !crate::vm_snapshots::is_high_risk(tool)
}

/// The canonical body a snapshot's id hashes: its version, the agent's CID, the issue time, and the allow list (sorted,
/// as it is sent), one field to a line.
fn body(cid: u64, issued_ms: u64, allow: &[String]) -> String {
    let mut body = format!("apz-policy-v1\ncid {cid}\nissued_ms {issued_ms}\n");
    for tool in allow {
        body.push_str("allow ");
        body.push_str(tool);
        body.push('\n');
    }
    body
}

/// The id of a snapshot: the SHA-256 hex of its canonical body.
fn id_of(cid: u64, issued_ms: u64, allow: &[String]) -> String {
    Sha256::digest(body(cid, issued_ms, allow).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Keeps a snapshot issued to agent `cid`, dropping the oldest past [`KEPT`].
fn keep(cid: u64, snapshot: PolicySnapshot) {
    let mut issued = lock(&ISSUED);
    let kept = issued.entry(cid).or_default();
    kept.push(snapshot);
    let excess = kept.len().saturating_sub(KEPT);
    kept.drain(..excess);
}

/// Issues a snapshot of `allow` for agent `cid` now: its id, and the hub's signature of the id with the hub's Weft key.
fn issue(app: &App, cid: u64, allow: Vec<String>) -> Result<PolicySnapshot, String> {
    let issued_ms = apz_plaza::now_ms();
    let id = id_of(cid, issued_ms, &allow);
    let key = crate::weft_hub::hub_key(app)?;
    let signature = URL_SAFE_NO_PAD.encode(key.sign(id.as_bytes()));
    let snapshot = PolicySnapshot {
        id,
        issued_ms,
        allow,
        signature,
    };
    keep(cid, snapshot.clone());
    Ok(snapshot)
}

/// The allow list agent `agent` has now, as the hub decides its calls.
fn current(app: &App, agent: &Agent) -> Vec<String> {
    let granted: Vec<String> = profile(app, agent)
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    let fallback = crate::project_settings::permission_mode(app, agent.cid);
    let (mode, _) = app.approvals.effective(agent.cid, Some(fallback));
    let held = app.control.paused() || crate::projects::held(app, agent).is_some();
    allowed(app.laws, agent.cid, &agent.role, &granted, mode, held)
}

/// Sends agent `cid` a snapshot as its policy line.
fn send(app: &App, cid: u64, policy: PolicySnapshot) -> Result<(), String> {
    let body = json!({"message": HostMessage::Policy { policy }});
    crate::fleet::send(app, cid, &body).map(|_| ())
}

/// The policy an agent's session starts with, sent right after its welcome: issued now, for what the agent may run.
pub fn at_hello(app: &App, cid: u64) -> Option<PolicySnapshot> {
    let _serial = lock(&PUSHING);
    let agent = app.agents.get(cid)?;
    let allow = current(app, &agent);
    match issue(app, cid, allow.clone()) {
        Ok(snapshot) => {
            lock(&SENT).insert(cid, allow);
            Some(snapshot)
        }
        Err(err) => {
            eprintln!("apid: agent {cid} has no policy: {err}");
            None
        }
    }
}

/// Pushes agent `cid` the snapshot of what it may run now, when that differs from the one it was last sent. An agent
/// that is offline or asleep has no session to send to: its next hello brings its snapshot.
pub fn push(app: &App, cid: u64) {
    let _serial = lock(&PUSHING);
    let Some(agent) = app.agents.get(cid) else {
        return;
    };
    if matches!(agent.status, Status::Offline | Status::Asleep) {
        return;
    }
    let allow = current(app, &agent);
    if lock(&SENT).get(&cid) == Some(&allow) {
        return;
    }
    match issue(app, cid, allow.clone()).and_then(|snapshot| send(app, cid, snapshot)) {
        Ok(()) => {
            lock(&SENT).insert(cid, allow);
        }
        Err(err) => eprintln!("apid: agent {cid} not sent its policy: {err}"),
    }
}

/// Pushes every agent the snapshot of what it may run now, where that changed (see [`push`]). Called where a change
/// the snapshot depends on is made: the laws and rules, STOP ALL and resume, a project's state.
pub fn push_all(app: &App) {
    for agent in app.agents.list() {
        push(app, agent.cid);
    }
}

/// Why a call of `tool` that agent `cid` reports as decided under policy `policy` is not one the hub issued that
/// policy for: the hub never issued it to the agent, or it does not allow the tool. `None` when the call is allowed.
pub fn violation(cid: u64, policy: &str, tool: &str) -> Option<String> {
    let issued = lock(&ISSUED);
    let Some(snapshot) = issued
        .get(&cid)
        .and_then(|kept| kept.iter().rev().find(|snapshot| snapshot.id == policy))
    else {
        return Some(format!(
            "{tool} was decided under policy {policy}, which the hub never issued to this agent"
        ));
    };
    if snapshot.allow.iter().any(|allowed| allowed == tool) {
        None
    } else {
        Some(format!(
            "{tool} ran under policy {policy}, which does not allow it"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laws::LAW_FILES;

    fn granted_to_builder() -> Vec<String> {
        crate::tools::offered_names("builder")
    }

    fn laws() -> Laws {
        Laws::from_files(
            LAW_FILES
                .iter()
                .map(|(name, text)| (name.to_string(), text.to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_tool_whose_input_a_law_reads_is_not_allowed_locally() {
        let laws = laws();
        // Commands and file writes are checked for the flags of their input (C-001c, C-002b).
        for tool in [
            "bash",
            "write_file",
            "edit_file",
            "install_packages",
            "fetch_url",
        ] {
            assert!(laws.tests_input(tool), "{tool} tests its input");
        }
        // A message reads its recipient, and a read-only tool no law reads the input of.
        assert!(laws.tests_input("plaza_send_message"));
        assert!(!laws.tests_input("glob"));
        assert!(!laws.tests_input("grep"));
        let local = allowed(
            &laws,
            7,
            "builder",
            &granted_to_builder(),
            Mode::Autonomous,
            false,
        );
        for tool in [
            "bash",
            "write_file",
            "edit_file",
            "install_packages",
            "fetch_url",
        ] {
            assert!(
                !local.iter().any(|t| t == tool),
                "{tool} is decided by the hub"
            );
        }
        assert!(local.iter().any(|t| t == "glob"), "{local:?}");
    }

    #[test]
    fn a_tool_the_hub_runs_or_settles_is_never_allowed_locally() {
        let granted: Vec<String> = [
            "plaza_tasks",
            "code_engine",
            "tool_package",
            "x_made_tool",
            "engine.exec",
            "checkpoint_list",
        ]
        .iter()
        .map(|tool| tool.to_string())
        .collect();
        let allowed = allowed(&laws(), 7, "builder", &granted, Mode::Autonomous, false);
        assert!(!allowed.iter().any(|t| t == "plaza_tasks"), "a host tool");
        assert!(!allowed.iter().any(|t| t == "code_engine"), "an engine");
        assert!(
            !allowed.iter().any(|t| t == "tool_package"),
            "a packaged tool"
        );
        assert!(
            !allowed.iter().any(|t| t == "x_made_tool"),
            "a library tool"
        );
        assert!(
            !allowed.iter().any(|t| t.starts_with("engine.")),
            "an engine request"
        );
    }

    #[test]
    fn nothing_is_allowed_locally_unless_the_agent_is_autonomous_and_not_held() {
        let laws = laws();
        let granted = granted_to_builder();
        assert!(!allowed(&laws, 7, "builder", &granted, Mode::Autonomous, false).is_empty());
        // Ask and read-only decide each call; a held agent (STOP ALL, or its project paused or over) runs nothing.
        assert!(allowed(&laws, 7, "builder", &granted, Mode::Ask, false).is_empty());
        assert!(allowed(&laws, 7, "builder", &granted, Mode::ReadOnly, false).is_empty());
        assert!(allowed(&laws, 7, "builder", &granted, Mode::Autonomous, true).is_empty());
    }

    #[test]
    fn the_snapshot_id_binds_the_agent_the_time_and_the_list() {
        let list = vec!["glob".to_string(), "grep".to_string()];
        let id = id_of(7, 100, &list);
        assert_eq!(id.len(), 64, "{id}");
        assert_eq!(
            id,
            id_of(7, 100, &list),
            "the same snapshot has the same id"
        );
        assert_ne!(id, id_of(8, 100, &list), "another agent's snapshot differs");
        assert_ne!(id, id_of(7, 101, &list), "a later snapshot differs");
        assert_ne!(id, id_of(7, 100, &list[..1]), "a shorter list differs");
    }

    #[test]
    fn a_report_outside_the_snapshot_is_a_violation_and_nothing_else_is() {
        let snapshot = PolicySnapshot {
            id: "policy-a".into(),
            issued_ms: 1,
            allow: vec!["glob".into()],
            signature: "sig".into(),
        };
        keep(9, snapshot);
        assert_eq!(violation(9, "policy-a", "glob"), None);
        assert!(
            violation(9, "policy-a", "bash")
                .unwrap()
                .contains("does not allow")
        );
        assert!(
            violation(9, "policy-b", "glob")
                .unwrap()
                .contains("never issued")
        );
        // Another agent's snapshot is not this agent's.
        assert!(
            violation(8, "policy-a", "glob")
                .unwrap()
                .contains("never issued")
        );
    }

    #[test]
    fn the_snapshots_kept_for_an_agent_are_bounded() {
        for n in 0..(KEPT as u64 + 3) {
            keep(
                11,
                PolicySnapshot {
                    id: format!("p{n}"),
                    issued_ms: n,
                    allow: Vec::new(),
                    signature: String::new(),
                },
            );
        }
        // The oldest are gone, so a report under one of them is "never issued"; the newest still names a snapshot.
        assert!(
            violation(11, "p0", "glob")
                .unwrap()
                .contains("never issued")
        );
        let newest = format!("p{}", KEPT as u64 + 2);
        assert!(
            violation(11, &newest, "glob")
                .unwrap()
                .contains("does not allow")
        );
    }
}
