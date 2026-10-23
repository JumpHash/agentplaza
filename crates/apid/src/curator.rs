//! The curator (PLAN 9.4, Engine 2.3): the session that turns what a project's finished tasks wrote in their journals,
//! and the memory proposals its sessions made, into the entries the project's memory keeps.
//!
//! - A task that ends is noted for its project ([`requested`], from `work_loop::close_task`). The project's curator runs
//!   while a task waits, and once a day while a proposal does ([`tick`]). Its session has no task: it is the project's,
//!   on the curator's model (a project's own, else the plaza's, else its first worker model).
//! - Its prompt is the journals of the tasks it was given, the proposals, and the index as it stands. It answers once,
//!   with `memory_land`: the entries it accepts (one fact each, typed), the proposals it rejects, and the contradictions
//!   it found. The entries land on main as one change (`project_memory::land`); a contradiction opens a memory card for
//!   the owner; the proposals the answer names are taken off the list, and the tasks it read are done with.
//! - Decisions do not come from here: the planner records them (`memory_record_decision`).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::Agent;
use crate::cards::{self, Choice, NewCard};
use crate::journal;
use crate::project_memory::{self, Entry, Proposal};
use crate::scheduler::Origin;
use crate::sessions::{Kind, NewSession, Session, State};
use crate::state::App;

/// The seat and the role of a project's curator; its sessions have no task.
pub const SEAT: &str = "curator";
/// The host tool the curator answers with.
pub const TOOLS: &[&str] = &["memory_land"];

/// How long a project waits after its curator's run started before a run for the tasks waiting on it.
pub const BACKOFF_MS: u64 = 10 * 60_000;
/// How often a project's curator runs for proposals alone.
pub const DAILY_MS: u64 = 24 * 60 * 60_000;
/// How many entries one answer may land.
const ENTRIES_MAX: usize = 50;

/// A project's curation: the finished tasks whose journals wait for its curator, and when its last run started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "memory_curation")]
pub struct Curation {
    #[table(key)]
    pub project: String,
    /// When the last run started (0: never).
    pub started_ms: u64,
    /// The finished tasks whose journals the next run reads.
    #[serde(default)]
    #[table(list)]
    pub tasks: Vec<String>,
    /// The tasks the running curator was given: its answer takes them off `tasks`.
    #[serde(default)]
    #[table(list)]
    pub reading: Vec<String>,
}

/// Every project's curation.
#[derive(Default)]
pub struct Store {
    inner: Mutex<BTreeMap<String, Curation>>,
}

impl Store {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Curation>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The project's curation, when it has one.
    pub fn get(&self, project: &str) -> Option<Curation> {
        self.lock().get(project).cloned()
    }

    /// Changes the project's curation, made on first use.
    fn with<R>(&self, project: &str, change: impl FnOnce(&mut Curation) -> R) -> R {
        let mut inner = self.lock();
        let curation = inner
            .entry(project.to_string())
            .or_insert_with(|| Curation {
                project: project.to_string(),
                started_ms: 0,
                tasks: Vec::new(),
                reading: Vec::new(),
            });
        change(curation)
    }

    /// Every curation's rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let inner = self.lock();
        RowSet::list(def(), inner.values())
    }

    /// Replaces the curations with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<Curation> = apz_store::table::load_list(conn, def())?;
        *self.lock() = stored.into_iter().map(|c| (c.project.clone(), c)).collect();
        Ok(())
    }
}

/// The curations' table: `memory_curation`, with its tasks and the tasks of its running run as child rows.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Curation)
}

/// `task` of `project` ended: its journal waits for the project's curator.
pub fn requested(app: &App, project: &str, task: &str) {
    app.curator.with(project, |c| note(c, task));
}

/// Notes `task` as waiting on the curation (once).
fn note(curation: &mut Curation, task: &str) {
    if !curation.tasks.iter().any(|t| t == task) {
        curation.tasks.push(task.to_string());
    }
}

/// Whether a project's curator runs now: a task waits and the last run started [`BACKOFF_MS`] ago, or the oldest
/// proposal waiting has waited a day since it was made or since the last run started, whichever is later.
pub fn due(tasks: usize, started_ms: u64, oldest_proposal_ms: Option<u64>, now: u64) -> bool {
    let daily =
        oldest_proposal_ms.is_some_and(|made| now.saturating_sub(started_ms.max(made)) >= DAILY_MS);
    (tasks > 0 && now.saturating_sub(started_ms) >= BACKOFF_MS) || daily
}

/// Starts the curator of each active project that is due (every tick of the work loop).
pub fn tick(app: &App, now: u64) {
    if !crate::roles_custom::exists(app, SEAT) {
        return;
    }
    for project in app.projects.list() {
        if !crate::work_loop::project_active(app, &project.id) || live(app, &project.id) {
            continue;
        }
        let (tasks, started) = app
            .curator
            .get(&project.id)
            .map_or((0, 0), |c| (c.tasks.len(), c.started_ms));
        let oldest = app
            .project_memory
            .proposals_of(&project.id)
            .iter()
            .map(|p| p.created_ms)
            .min();
        if due(tasks, started, oldest, now) {
            start(app, &project.id, now);
        }
    }
}

/// Whether `project` has a curator session that has not ended.
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

/// The model a project's curator runs on: the project's for the role, else the plaza's, else the first worker model
/// the plaza lists (the cheapest it runs).
pub fn curator_model(app: &App, project: &str) -> Option<String> {
    let settings = app.settings.get();
    app.project_settings
        .get(project)
        .role_models
        .get(SEAT)
        .cloned()
        .filter(|m| !m.is_empty())
        .or_else(|| {
            settings
                .role_models
                .get(SEAT)
                .cloned()
                .filter(|m| !m.is_empty())
        })
        .or_else(|| settings.model_policy.worker.first().cloned())
        .or_else(|| Some(app.settings.resolve(SEAT, None)))
}

/// Starts `project`'s curator: a session with no task, on a VM the node starts. Its run reads the tasks waiting now.
fn start(app: &App, project: &str, now: u64) {
    app.curator.with(project, |c| {
        c.started_ms = now;
        c.reading = c.tasks.clone();
    });
    let model = curator_model(app, project);
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
    app.evals.begin(&session.id, "distill", "curate", 1);
    // The curator's run is a run of the engine's `distill` recipe (recipes/runs.rs); its session ends it.
    app.recipe_runs.start_engine(
        "distill",
        "curator",
        None,
        Some(project),
        Some(project),
        now,
    );
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

/// The curator's VM said hello: its work goes out, with the prompt its run was given. A run that ends without answering
/// leaves its tasks waiting for the next one.
pub fn start_work(app: &App, session: &Session, cid: u64) {
    let prompt = prompt(app, &session.project);
    let work = app.work.enqueue(
        app,
        cid,
        apz_proto::wire::WorkKind::Task,
        format!("curate-{}", session.project),
        prompt,
        Origin::default(),
    );
    let _ = app.sessions.update(&session.id, |s| s.work = Some(work));
    app.bus.publish("session", &app.sessions.get(&session.id));
}

/// The curator's prompt: the journals of the tasks it reads, the proposals waiting, and the index as it stands.
fn prompt(app: &App, project: &str) -> String {
    let reading = app
        .curator
        .get(project)
        .map(|c| c.reading)
        .unwrap_or_default();
    let journals: Vec<String> = reading.iter().map(|task| journal_text(app, task)).collect();
    let proposals = app.project_memory.proposals_of(project);
    let index = project_memory::index_text(&project_memory::entries(app, project));
    compose(&journals.concat(), &proposals, &index)
}

/// A task's journal as the curator reads it: its brief, its notes and its latest handover.
fn journal_text(app: &App, task_id: &str) -> String {
    let mut out = match app.plaza.tasks.find(task_id) {
        Some(task) => journal::brief_text(&task),
        None => format!("Task {task_id}\n"),
    };
    match app.journal.get(task_id) {
        Some(j) => {
            out.push_str(&journal::notes_text(&j.notes));
            if let Some(handover) = j.handovers.last() {
                out.push_str(&format!(
                    "Latest handover:\n{}",
                    journal::handover_text(handover)
                ));
            }
        }
        None => out.push_str("No journal notes.\n"),
    }
    out
}

/// The prompt's text around what the run reads.
fn compose(journals: &str, proposals: &[Proposal], index: &str) -> String {
    let mut out = String::from(
        "You are this project's curator. Its tasks ended and left journals, and its sessions proposed entries for its \
         memory. Decide what the project's memory keeps.\n\nThe journals of the tasks you were given:\n",
    );
    out.push_str(if journals.is_empty() {
        "None.\n"
    } else {
        journals
    });
    out.push_str("\nThe proposals:\n");
    if proposals.is_empty() {
        out.push_str("None.\n");
    }
    for p in proposals {
        let anchors = if p.anchors.is_empty() {
            String::new()
        } else {
            format!(" (anchors: {})", p.anchors.join(", "))
        };
        out.push_str(&format!(
            "- {} {}{anchors}: {} Why: {}\n",
            p.id, p.kind, p.text, p.why
        ));
    }
    out.push_str("\nThe project's memory as it stands (its index):\n");
    out.push_str(if index.is_empty() { "Empty.\n" } else { index });
    out.push_str(
        "\nKeep an entry only when it holds a reason, a trap, a how-to, or a fact the code does not show: one fact per \
         entry, in a few sentences. Drop what the code, its history or Weft already records, and trivia; merge what \
         says the same thing. Name the proposal each accepted entry came from, and give each rejected proposal a why. \
         Where two entries in the memory contradict each other, name them as a contradiction.\n\nAnswer with one call \
         to memory_land: entries (type, text, anchors, links, proposal), rejected (proposal, why), contradictions \
         (ids, why). Then end your turn.\n",
    );
    out
}

/// What the curator answers: the entries it accepts, the proposals it rejects, and the contradictions it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub entries: Vec<Accepted>,
    pub rejected: Vec<(String, String)>,
    pub contradictions: Vec<(Vec<String>, String)>,
}

/// One entry the curator accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub kind: String,
    pub text: String,
    pub anchors: Vec<String>,
    pub links: Vec<String>,
    pub proposal: Option<String>,
}

/// The answer `memory_land` gives, checked: each entry has a type and a text (and the anchors and links its checks
/// allow), each rejection a proposal and a why, each contradiction two or more ids and a why.
pub fn answer_from(input: &Value) -> Result<Answer, String> {
    let items = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let text = |item: &Value, key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let entries_in = items("entries");
    if entries_in.len() > ENTRIES_MAX {
        return Err(format!("entries takes at most {ENTRIES_MAX}"));
    }
    let mut entries = Vec::new();
    for item in &entries_in {
        let kind = text(item, "type");
        if !project_memory::KINDS.contains(&kind.as_str()) {
            return Err(format!(
                "entry type must be one of {}",
                project_memory::KINDS.join(", ")
            ));
        }
        let proposal = Some(text(item, "proposal")).filter(|p| !p.is_empty());
        let links = item
            .get("links")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        entries.push(Accepted {
            kind,
            text: project_memory::text_of(&text(item, "text"))?,
            anchors: project_memory::anchors_from(item)?,
            links,
            proposal,
        });
    }
    let mut rejected = Vec::new();
    for item in items("rejected") {
        let (proposal, why) = (text(&item, "proposal"), text(&item, "why"));
        if proposal.is_empty() || why.is_empty() {
            return Err("a rejection names its proposal and why".into());
        }
        rejected.push((proposal, why));
    }
    let mut contradictions = Vec::new();
    for item in items("contradictions") {
        let ids: Vec<String> = item
            .get("ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let why = text(&item, "why");
        if ids.len() < 2 || why.is_empty() {
            return Err("a contradiction names two or more ids and why".into());
        }
        contradictions.push((ids, why));
    }
    Ok(Answer {
        entries,
        rejected,
        contradictions,
    })
}

/// `memory_land`: the curator's one answer. The accepted entries land on main as one change; the proposals it answered
/// leave the list, the tasks it read are done with, each contradiction opens a memory card for the owner, and the
/// project's chat is told what landed.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let session = app
        .sessions
        .by_cid(agent.cid)
        .filter(|s| s.seat.as_deref() == Some(SEAT) && s.task.is_none())
        .ok_or_else(|| "only the project's curator lands memory".to_string())?;
    let project = session.project.clone();
    let answer = answer_from(input)?;
    let proposals = app.project_memory.proposals_of(&project);
    let answered: Vec<String> = answer
        .entries
        .iter()
        .filter_map(|e| e.proposal.clone())
        .chain(answer.rejected.iter().map(|(id, _)| id.clone()))
        .collect();
    if let Some(unknown) = answered
        .iter()
        .find(|id| !proposals.iter().any(|p| &p.id == *id))
    {
        return Err(format!("no proposal {unknown} in this project"));
    }
    let now = apz_plaza::now_ms();
    let mut landed = Vec::new();
    let mut change = None;
    if !answer.entries.is_empty() {
        let drafts: Vec<Entry> = answer
            .entries
            .iter()
            .map(|e| {
                let mut entry = project_memory::draft(&project, &e.kind, e.text.clone());
                entry.anchors = e.anchors.clone();
                entry.links = e.links.clone();
                entry.provenance_session = session.id.clone();
                entry
            })
            .collect();
        let title = format!("The curator's memory: {} entries", drafts.len());
        let landing = project_memory::land(app, &project, &title, drafts, &[])?;
        landed = landing.ids;
        change = Some(landing.change);
    }
    app.project_memory.take_proposals(&answered);
    app.curator.with(&project, |c| {
        let reading = c.reading.clone();
        c.tasks.retain(|t| !reading.contains(t));
        c.reading.clear();
    });
    for (ids, why) in &answer.contradictions {
        open_card(
            app,
            &project,
            ids,
            &format!(
                "The memory contradicts itself: {}. {why} Keep both, or retire the ones named?",
                ids.join(" and ")
            ),
            now,
        );
    }
    let chat = crate::work_loop::project_chat(app, &project, now);
    let report = match &change {
        Some(change) => format!("The curator landed {} as {change}.", landed.join(", ")),
        None => "The curator landed nothing.".to_string(),
    };
    crate::work_loop::notice(app, &chat, &report, now);
    Ok(format!(
        "Answered: {} entries land as {}, {} proposals rejected, {} contradictions to the owner. End your turn now.",
        landed.len(),
        change.as_deref().unwrap_or("no change"),
        answer.rejected.len(),
        answer.contradictions.len()
    ))
}

/// Opens the owner's memory card about the entries `ids` of `project`: `keep` leaves them, `retire` retires them.
fn open_card(app: &App, project: &str, ids: &[String], title: &str, now: u64) {
    let chat = crate::work_loop::project_chat(app, project, now);
    crate::work_loop::open_card(
        app,
        NewCard {
            kind: cards::Kind::Memory,
            chat: Some(chat),
            project: Some(project.to_string()),
            task: None,
            session: None,
            subject: format!("memory:{project}:{}", ids.join(",")),
            title: title.to_string(),
            body: None,
            choices: vec![
                Choice {
                    id: "keep".into(),
                    label: "Keep them".into(),
                    detail: Some("Leave the entries as they are".into()),
                    takes_amount: false,
                },
                Choice {
                    id: "retire".into(),
                    label: "Retire them".into(),
                    detail: Some("Retire the entries the card names".into()),
                    takes_amount: false,
                },
            ],
        },
        now,
    );
}

/// The owner decided `id` in the project's chat: a memory card asks the owner to keep the decision or retire it.
pub fn ask_owner(app: &App, project: &str, id: &str, title: &str, now: u64) {
    open_card(
        app,
        project,
        &[id.to_string()],
        &format!("The owner decided {id}: {title}. Keep it, or retire it?"),
        now,
    );
}

/// The owner's answer to a memory card. `retire` retires the entries the card names, as a change to main; anything
/// else leaves them. Returns what happened, for the card's answer.
pub fn card_answered(app: &App, card: &cards::Card) -> String {
    if card.answer.as_deref() != Some("retire") {
        return "recorded".into();
    }
    let mut parts = card.subject.splitn(3, ':');
    let (Some("memory"), Some(project), Some(ids)) = (parts.next(), parts.next(), parts.next())
    else {
        return "no such memory card".into();
    };
    let ids: Vec<&str> = ids.split(',').collect();
    let retired: Vec<Entry> = project_memory::entries(app, project)
        .into_iter()
        .filter(|e| ids.contains(&e.id.as_str()))
        .map(|mut e| {
            e.status = "retired".into();
            e
        })
        .collect();
    if retired.is_empty() {
        return "no such entry".into();
    }
    let title = format!("The owner retires {}", ids.join(", "));
    match project_memory::land(app, project, &title, retired, &[]) {
        Ok(landing) => format!("retired as {}", landing.change),
        Err(why) => format!("not retired: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn proposal(id: &str, kind: &str, text: &str) -> Proposal {
        Proposal {
            id: id.into(),
            project: "pr-1".into(),
            kind: kind.into(),
            text: text.into(),
            anchors: vec!["sym:apid::sleep::wake_with".into()],
            why: "seen twice".into(),
            by_cid: 7,
            by: "builder #7".into(),
            created_ms: 1,
        }
    }

    #[test]
    fn a_curator_runs_for_a_waiting_task_after_the_backoff_and_for_a_proposal_a_day_after_it_or_its_last_run()
     {
        let now = 10 * DAILY_MS;
        assert!(due(1, 0, None, now), "a task waiting, never run");
        assert!(!due(1, now - 1000, None, now), "a run just started");
        assert!(due(1, now - BACKOFF_MS, None, now));
        assert!(!due(0, 0, None, now), "nothing to curate");
        let made = now - 1000;
        assert!(
            !due(0, 0, Some(made), now),
            "a proposal waits a day from when it was made"
        );
        assert!(due(0, 0, Some(now - DAILY_MS), now));
        assert!(
            !due(0, now - 1000, Some(now - DAILY_MS), now),
            "a run just started takes the day"
        );
        assert!(due(0, now - DAILY_MS, Some(now - DAILY_MS * 2), now));
    }

    #[test]
    fn a_task_is_noted_for_its_project_once_and_a_run_takes_only_what_it_read() {
        let mut curation = Curation {
            project: "pr-1".into(),
            started_ms: 0,
            tasks: Vec::new(),
            reading: Vec::new(),
        };
        note(&mut curation, "T-1");
        note(&mut curation, "T-1");
        note(&mut curation, "T-2");
        assert_eq!(curation.tasks, ["T-1", "T-2"]);
        // The run reads T-1 and T-2; T-3 ends while it runs and waits for the next one.
        curation.reading = curation.tasks.clone();
        note(&mut curation, "T-3");
        let reading = curation.reading.clone();
        curation.tasks.retain(|t| !reading.contains(t));
        curation.reading.clear();
        assert_eq!(curation.tasks, ["T-3"]);
    }

    #[test]
    fn the_prompt_carries_the_journals_the_proposals_and_the_index() {
        let text = compose(
            "Task T-1: Draft\n- decision n1: keep one file Reason: one place\n",
            &[proposal("mp1", "pitfall", "The wake never starts.")],
            "- D-0001 decision: Sessions, not named agents.",
        );
        assert!(
            text.contains("- decision n1: keep one file Reason: one place"),
            "{text}"
        );
        assert!(
            text.contains("- mp1 pitfall (anchors: sym:apid::sleep::wake_with): The wake never starts. Why: seen twice"),
            "{text}"
        );
        assert!(
            text.contains("- D-0001 decision: Sessions, not named agents."),
            "{text}"
        );
        assert!(text.contains("memory_land"), "{text}");
        let empty = compose("", &[], "");
        assert!(
            empty.contains("None.") && empty.contains("Empty."),
            "{empty}"
        );
    }

    #[test]
    fn an_answer_checks_its_entries_rejections_and_contradictions() {
        let answer = answer_from(&json!({
            "entries": [
                { "type": "decision", "text": "Keep one file.\n\nReason: one place.", "anchors": ["sym:apid::journal::run"] },
                { "type": "pitfall", "text": "The wake never starts.", "proposal": "mp1", "links": ["D-0001"] }
            ],
            "rejected": [{ "proposal": "mp2", "why": "the code says it" }],
            "contradictions": [{ "ids": ["P-0001", "P-0002"], "why": "both name the wake" }]
        }))
        .unwrap();
        assert_eq!(answer.entries.len(), 2);
        assert_eq!(answer.entries[0].kind, "decision");
        assert_eq!(answer.entries[0].proposal, None);
        assert_eq!(answer.entries[1].proposal.as_deref(), Some("mp1"));
        assert_eq!(answer.entries[1].links, ["D-0001"]);
        assert_eq!(
            answer.rejected,
            [("mp2".to_string(), "the code says it".to_string())]
        );
        assert_eq!(answer.contradictions[0].0, ["P-0001", "P-0002"]);

        let refused = |input: Value| answer_from(&input).unwrap_err();
        assert!(
            refused(json!({ "entries": [{ "type": "rumour", "text": "x" }] }))
                .starts_with("entry type")
        );
        assert_eq!(
            refused(json!({ "entries": [{ "type": "fact", "text": "" }] })),
            "text is required"
        );
        assert_eq!(
            refused(json!({ "rejected": [{ "proposal": "mp1" }] })),
            "a rejection names its proposal and why"
        );
        assert_eq!(
            refused(json!({ "contradictions": [{ "ids": ["P-1"], "why": "x" }] })),
            "a contradiction names two or more ids and why"
        );
        let many: Vec<Value> = (0..=ENTRIES_MAX)
            .map(|i| json!({ "type": "fact", "text": format!("fact {i}") }))
            .collect();
        assert!(refused(json!({ "entries": many })).starts_with("entries takes at most"));
    }
}
