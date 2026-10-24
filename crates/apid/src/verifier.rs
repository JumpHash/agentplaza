//! The verifier (PLAN 9.6, Engine 2.4): the session that checks the memory entries a landing flagged.
//!
//! - A landing that touches an entry's anchors flags the entry (`project_memory::landed`). A project with flagged entries
//!   gets its verifier once the last one ended [`BACKOFF_MS`] ago ([`tick`]). Its session has no task: it is the
//!   project's, on the curator's model.
//! - Its prompt is the flagged entries and the diffs of the landings that flagged them (capped). It answers once, with
//!   `memory_verify`: each entry it names is confirmed (checked again, so it gets a new `verified`), rewritten (a
//!   superseding entry, and the old one superseded) or retired. The answer lands as one change on main; the entries it
//!   does not name stay flagged for the next run.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

use crate::agents::Agent;
use crate::project_memory::{self, Entry};
use crate::scheduler::Origin;
use crate::sessions::{Kind, NewSession, Session, State};
use crate::state::App;

/// The seat and the role of a project's verifier; its sessions have no task.
pub const SEAT: &str = "verifier";

/// The host tools this seat answers with.
pub const TOOLS: &[&str] = &["memory_verify"];

/// How long after a verifier started the next one may start for the same project.
pub const BACKOFF_MS: u64 = 10 * 60_000;

/// The most verdicts one answer names.
const VERDICTS_MAX: usize = 50;

/// The most diff text the prompt carries for each landing that flagged entries.
const DIFF_MAX: usize = 24 * 1024;

/// When each project's verifier last started. Kept in this process: a restart starts the verifier again at its next tick.
#[derive(Default)]
pub struct Store {
    started: Mutex<BTreeMap<String, u64>>,
}

impl Store {
    fn started_ms(&self, project: &str) -> u64 {
        lock(&self.started).get(project).copied().unwrap_or(0)
    }

    fn set_started(&self, project: &str, now: u64) {
        lock(&self.started).insert(project.to_string(), now);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The action an answer takes on a flagged entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Confirm,
    Rewrite,
    Retire,
}

/// One verdict of the answer: the entry it is about and what is done to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub id: String,
    pub action: Action,
    /// The superseding entry's text (a rewrite only).
    pub text: String,
    /// The superseding entry's anchors (a rewrite only; none keeps the old entry's).
    pub anchors: Vec<String>,
}

/// The entries of `project` that are flagged, by type then id.
fn flagged_of(app: &App, project: &str) -> Vec<Entry> {
    let mut flagged: Vec<Entry> = project_memory::entries(app, project)
        .into_iter()
        .filter(|e| e.status == "flagged")
        .collect();
    flagged.sort_by(|a, b| a.id.cmp(&b.id));
    flagged
}

/// Starts the verifier of each active project that has flagged entries and is not backing off (every tick of the loop).
pub fn tick(app: &App, now: u64) {
    if !crate::roles_custom::exists(app, SEAT) {
        return;
    }
    for project in app.projects.list() {
        if !crate::work_loop::project_active(app, &project.id) || live(app, &project.id) {
            continue;
        }
        let backing_off = now.saturating_sub(app.verifier.started_ms(&project.id)) < BACKOFF_MS;
        if backing_off || flagged_of(app, &project.id).is_empty() {
            continue;
        }
        start(app, &project.id, now);
    }
}

/// Whether `project` has a verifier session that has not ended.
fn live(app: &App, project: &str) -> bool {
    app.sessions
        .list(&crate::sessions::Filter {
            project: Some(project.to_string()),
            live: true,
            ..Default::default()
        })
        .iter()
        .any(|s| s.seat.as_deref() == Some(SEAT))
}

/// Starts `project`'s verifier: a session with no task, on a VM the node starts.
fn start(app: &App, project: &str, now: u64) {
    app.verifier.set_started(project, now);
    let model = crate::curator::curator_model(app, project);
    let session = app.sessions.create(
        NewSession {
            project: project.to_string(),
            task: None,
            seat: Some(SEAT.into()),
            kind: Kind::Step,
            parent: None,
            role: SEAT.into(),
            engine: crate::work_loop::ENGINE.into(),
            model: model.clone(),
            attempt: 1,
        },
        now,
    );
    app.evals.begin(&session.id, "distill", 1, "verify", 1);
    let _ = app.sessions.set_state(&session.id, State::Placing, now);
    match crate::agents::spawn_agents(app, SEAT, 1, model.as_deref(), None, Some(project)) {
        Ok(started) => match started["cids"].get(0).and_then(Value::as_u64) {
            Some(cid) => {
                let node = started["node"].as_str().map(str::to_string);
                let _ = app.sessions.update(&session.id, |s| {
                    s.cid = Some(cid);
                    s.node = node;
                });
                let _ = app.sessions.set_state(&session.id, State::Forking, now);
                app.bus.publish("session", &app.sessions.get(&session.id));
            }
            None => crate::work_loop::fail_session(app, &session, "the node started no VM", now),
        },
        Err(why) => crate::work_loop::fail_session(app, &session, &why, now),
    }
}

/// The verifier's VM said hello: its work goes out, with the prompt its run was given.
pub fn start_work(app: &App, session: &Session, cid: u64) {
    let prompt = prompt(app, &session.project);
    let work = app.work.enqueue(
        app,
        cid,
        apz_proto::wire::WorkKind::Task,
        format!("verify-{}", session.project),
        prompt,
        Origin::default(),
    );
    let _ = app.sessions.update(&session.id, |s| s.work = Some(work));
    app.bus.publish("session", &app.sessions.get(&session.id));
}

/// The verifier's prompt: the flagged entries, and the diffs of the landings that flagged them.
fn prompt(app: &App, project: &str) -> String {
    let flagged = flagged_of(app, project);
    let mut changes: Vec<String> = flagged
        .iter()
        .map(|e| e.provenance_change.clone())
        .filter(|c| !c.is_empty())
        .collect();
    changes.sort();
    changes.dedup();
    let diffs: Vec<String> = changes.iter().map(|c| diff_text(app, c)).collect();
    compose(&flagged, &diffs.concat())
}

/// A landed change's diff as Weft gives it, cut to `DIFF_MAX` bytes; empty when Weft does not answer.
fn diff_text(app: &App, change: &str) -> String {
    let Ok(rev) = project_memory::landed_revision(app, change) else {
        return String::new();
    };
    let Ok(diff) = app.weft.get(&format!("/revisions/{rev}/diff")) else {
        return String::new();
    };
    let text = format!("The diff of {change}:\n{diff}\n");
    let mut end = text.len().min(DIFF_MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// The prompt's text around the flagged entries and the diffs.
fn compose(flagged: &[Entry], diffs: &str) -> String {
    let mut out = String::from(
        "You are this project's verifier. Landed changes touched code that these memory entries are about, so the \
         entries are flagged until someone checks them against the code as it is now. Check each one.\n\nThe flagged \
         entries:\n",
    );
    if flagged.is_empty() {
        out.push_str("None.\n");
    }
    for e in flagged {
        out.push_str(&format!(
            "- {} {} (anchors: {}): {}\n",
            e.id,
            e.kind,
            e.anchors.join(", "),
            e.body
        ));
    }
    out.push_str("\nThe changes that flagged them:\n");
    out.push_str(if diffs.is_empty() { "None.\n" } else { diffs });
    out.push_str(
        "\nAnswer once with memory_verify: one verdict per entry you check. confirm: the entry is still true. rewrite: \
         it is not, and text is its replacement (one fact, a few sentences); the old entry is superseded. retire: it is \
         no longer true and has no replacement. An entry you do not name stays flagged.\n",
    );
    out
}

/// The verdicts `input` names: each with an action, an entry, and for a rewrite its text (within the memory's limits).
pub fn verdicts_from(input: &Value) -> Result<Vec<Verdict>, String> {
    let items = input
        .get("verdicts")
        .and_then(Value::as_array)
        .ok_or("verdicts is required")?;
    if items.len() > VERDICTS_MAX {
        return Err(format!("verdicts takes at most {VERDICTS_MAX} entries"));
    }
    let mut verdicts: Vec<Verdict> = Vec::new();
    for item in items {
        let text = |key: &str| {
            item.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let id = text("id");
        if id.is_empty() {
            return Err("every verdict names its entry".into());
        }
        if verdicts.iter().any(|v| v.id == id) {
            return Err(format!("entry {id} is named twice"));
        }
        let action = match text("action").as_str() {
            "confirm" => Action::Confirm,
            "rewrite" => Action::Rewrite,
            "retire" => Action::Retire,
            other => {
                return Err(format!(
                    "{other:?} is not an action: confirm, rewrite or retire"
                ));
            }
        };
        let rewritten = if action == Action::Rewrite {
            project_memory::text_of(&text("text"))?
        } else {
            String::new()
        };
        verdicts.push(Verdict {
            id,
            action,
            text: rewritten,
            anchors: project_memory::anchors_from(item)?,
        });
    }
    Ok(verdicts)
}

/// `memory_verify`: the verifier's one answer. The verdicts land on main as one change; the flagged entries they name
/// leave the flag, and the project's chat is told what changed.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let session = app
        .sessions
        .by_cid(agent.cid)
        .filter(|s| s.seat.as_deref() == Some(SEAT) && s.task.is_none())
        .ok_or_else(|| "only the project's verifier answers for memory".to_string())?;
    let project = session.project.clone();
    let verdicts = verdicts_from(input)?;
    let flagged = flagged_of(app, &project);
    let mut writes: Vec<Entry> = Vec::new();
    let (mut confirmed, mut rewritten, mut retired) = (0, 0, 0);
    for verdict in &verdicts {
        let old = flagged
            .iter()
            .find(|e| e.id == verdict.id)
            .ok_or_else(|| format!("no flagged entry {} in this project", verdict.id))?;
        match verdict.action {
            Action::Confirm => {
                let mut entry = old.clone();
                entry.status = "active".into();
                entry.verified_commit.clear();
                entry.verified_date.clear();
                writes.push(entry);
                confirmed += 1;
            }
            Action::Rewrite => {
                let mut entry = project_memory::draft(&project, &old.kind, verdict.text.clone());
                entry.anchors = if verdict.anchors.is_empty() {
                    old.anchors.clone()
                } else {
                    verdict.anchors.clone()
                };
                entry.links = old.links.clone();
                entry.supersedes = vec![old.id.clone()];
                entry.provenance_session = session.id.clone();
                writes.push(entry);
                let mut superseded = old.clone();
                superseded.status = "superseded".into();
                writes.push(superseded);
                rewritten += 1;
            }
            Action::Retire => {
                let mut entry = old.clone();
                entry.status = "retired".into();
                writes.push(entry);
                retired += 1;
            }
        }
    }
    let now = apz_plaza::now_ms();
    let mut change = "no change".to_string();
    if !writes.is_empty() {
        let title = format!(
            "The verifier's check: {confirmed} confirmed, {rewritten} rewritten, {retired} retired"
        );
        change = project_memory::land(app, &project, &title, writes, &[])?.change;
        let chat = crate::work_loop::project_chat(app, &project, now);
        let report = format!(
            "The verifier checked the memory: {confirmed} confirmed, {rewritten} rewritten, {retired} retired, as {change}."
        );
        crate::work_loop::notice(app, &chat, &report, now);
    }
    Ok(format!(
        "Answered: {confirmed} confirmed, {rewritten} rewritten, {retired} retired, as {change}. End your turn now."
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn verdicts_name_an_action_an_entry_and_a_rewrite_its_text() {
        let verdicts = verdicts_from(&json!({ "verdicts": [
            { "id": "P-0017", "action": "confirm", "why": "still the case" },
            { "id": "P-0018", "action": "rewrite", "text": "The move waits for the claim.", "anchors": ["sym:apid::sleep::wake_with"] },
            { "id": "P-0019", "action": "retire", "why": "the function is gone" }
        ] }))
        .expect("a valid answer");
        let actions: Vec<Action> = verdicts.iter().map(|v| v.action).collect();
        assert_eq!(actions, [Action::Confirm, Action::Rewrite, Action::Retire]);
        assert_eq!(verdicts[1].text, "The move waits for the claim.");
        assert_eq!(verdicts[1].anchors, ["sym:apid::sleep::wake_with"]);
    }

    #[test]
    fn a_bad_answer_is_refused_with_its_reason() {
        let refused = |input: Value| verdicts_from(&input).expect_err("refused");
        assert!(refused(json!({})).contains("verdicts is required"));
        assert!(
            refused(json!({ "verdicts": [{ "id": "P-1", "action": "delete" }] }))
                .contains("not an action")
        );
        assert!(
            refused(json!({ "verdicts": [{ "action": "confirm" }] })).contains("names its entry")
        );
        assert!(
            refused(json!({ "verdicts": [
            { "id": "P-1", "action": "confirm" }, { "id": "P-1", "action": "retire" }
        ] }))
            .contains("named twice")
        );
        assert!(
            refused(json!({ "verdicts": [{ "id": "P-1", "action": "rewrite", "text": "" }] }))
                .contains("text is required")
        );
    }

    #[test]
    fn the_prompt_names_each_flagged_entry_and_the_answer_tool() {
        let mut entry =
            project_memory::draft("p", "pitfall", "The move waits for the claim.".into());
        entry.id = "P-0017".into();
        entry.anchors = vec!["sym:apid::sleep::wake_with".into()];
        let text = compose(&[entry], "The diff of c1:\n+ wake\n");
        assert!(
            text.contains("- P-0017 pitfall (anchors: sym:apid::sleep::wake_with): The move waits"),
            "{text}"
        );
        assert!(text.contains("memory_verify"), "{text}");
        assert!(text.contains("The diff of c1"), "{text}");
    }
}
