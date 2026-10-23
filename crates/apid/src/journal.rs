//! The task journal (PLAN 9.2, Engine 2.1): what a task's sessions decided, what they tried and dropped, what is still
//! open, and each step session's handover.
//!
//! - A session writes its task's journal through four host tools: `journal_note` (a decision with its reason, a dead
//!   end with why it failed, an open question with its thread), `handover_write` (where things stand, what is done,
//!   what is next, what to check first) and the readers `journal_read` and `brief`. Every agent is offered them.
//! - The harness asks a step session for its handover before it ends (`handover_write`, unless its output says the
//!   task is done), and the next session of the task starts from the brief, the notes and the last handover
//!   (work_loop.rs).
//! - The records live in memory, keyed by task; the saver keeps them in `plaza.db` (table `journal`, with its notes and
//!   handovers as children). A task's journal is kept with the task.
//!
//! Search is a word match over one task's notes (`journal_read` with `query`), best match first: the FTS5 index in
//! plaza.db answers it (store/search.rs), and a case-insensitive match in memory answers it when the index cannot.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents::Agent;
use crate::state::App;

/// The host tools the journal serves.
pub const TOOLS: &[&str] = &["journal_note", "journal_read", "handover_write", "brief"];

/// What a note records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A choice, with the reason for it.
    Decision,
    /// Something tried that failed, with why.
    DeadEnd,
    /// A question still open, with the thread it was asked in.
    OpenQuestion,
}

impl Kind {
    /// The kind's name in the tool and in what the journal shows.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Decision => "decision",
            Kind::DeadEnd => "dead end",
            Kind::OpenQuestion => "open question",
        }
    }
}

/// One note of a task's journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Note {
    /// Its number on the task (1, 2, …).
    pub seq: u64,
    pub kind: Kind,
    /// The session that wrote it.
    pub session: Option<String>,
    #[table(content)]
    pub text: String,
    /// A decision's reason, a dead end's why.
    #[table(content)]
    pub reason: Option<String>,
    /// An open question's thread, when it was asked in one.
    pub thread: Option<String>,
    pub at_ms: u64,
}

/// One session's handover: where the task stands, for the session that starts next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Handover {
    /// Its number on the task (1, 2, …).
    pub seq: u64,
    /// The session that wrote it.
    pub session: String,
    #[table(content)]
    pub stands: String,
    #[table(content)]
    pub done: String,
    #[table(content)]
    pub next: String,
    #[table(content)]
    pub check_first: String,
    pub at_ms: u64,
}

/// One task's journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "journal")]
pub struct Journal {
    /// The task's id.
    #[table(key)]
    pub task: String,
    #[table(child)]
    pub notes: Vec<Note>,
    #[table(child)]
    pub handovers: Vec<Handover>,
}

/// Every task's journal.
#[derive(Default)]
pub struct Journals {
    inner: Mutex<BTreeMap<String, Journal>>,
}

impl Journals {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Journal>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The task's journal, when anything was written to it.
    pub fn get(&self, task: &str) -> Option<Journal> {
        self.lock().get(task).cloned()
    }

    /// Adds a note to the task's journal, numbering it; returns the number.
    pub fn add_note(&self, task: &str, mut note: Note) -> u64 {
        let mut inner = self.lock();
        let journal = entry(&mut inner, task);
        note.seq = journal.notes.len() as u64 + 1;
        let seq = note.seq;
        journal.notes.push(note);
        seq
    }

    /// Adds a handover to the task's journal, numbering it; returns the number.
    pub fn add_handover(&self, task: &str, mut handover: Handover) -> u64 {
        let mut inner = self.lock();
        let journal = entry(&mut inner, task);
        handover.seq = journal.handovers.len() as u64 + 1;
        let seq = handover.seq;
        journal.handovers.push(handover);
        seq
    }

    /// Every journal's tables, as the saver writes them.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        apz_store::RowSet::list(def(), inner.values())
    }

    /// Replaces the journals with what the store holds (at start).
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Journal> = apz_store::table::load_list(conn, def())?;
        *self.lock() = stored.into_iter().map(|j| (j.task.clone(), j)).collect();
        Ok(())
    }
}

/// The task's journal, created empty when it has none yet.
fn entry<'a>(inner: &'a mut BTreeMap<String, Journal>, task: &str) -> &'a mut Journal {
    inner.entry(task.to_string()).or_insert_with(|| Journal {
        task: task.to_string(),
        notes: Vec::new(),
        handovers: Vec::new(),
    })
}

/// The journal's tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Journal)
}

/// Runs journal tool `name` for `agent`. The tools that write act for the agent's session's task; the readers take a
/// task the input names (in the agent's project) or that task.
pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    let now = apz_plaza::now_ms();
    match name {
        "journal_note" => {
            let (session, task) = writing(app, agent)?;
            let note = note_from_input(input)?;
            let label = note.kind.label();
            let seq = app.journal.add_note(
                &task,
                Note {
                    session: Some(session.id),
                    at_ms: now,
                    ..note
                },
            );
            // Saved now, not on the saver's next pass, so the note is in the index before anyone searches for it.
            crate::store::saver::save_now(app, "journal");
            Ok(format!("Noted: {label} n{seq} on {task}."))
        }
        "handover_write" => {
            let (session, task) = writing(app, agent)?;
            let handover = handover_from_input(input)?;
            let text = handover_text(&handover);
            app.journal.add_handover(
                &task,
                Handover {
                    session: session.id.clone(),
                    at_ms: now,
                    ..handover
                },
            );
            // The session's handover is what the next session of its seat starts from (work_loop.rs).
            let _ = app
                .sessions
                .update(&session.id, |s| s.handover = Some(text));
            Ok(format!("Handover recorded for {task}. End your turn now."))
        }
        "journal_read" => {
            let task = named_task(app, agent, input)?;
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .filter(|query| !query.trim().is_empty());
            // The index's answer, best match first; when it cannot answer, the notes are matched in memory.
            let indexed = query.and_then(|query| {
                app.store
                    .db(crate::store::PLAZA)
                    .read(|conn| crate::store::search::journal_notes(conn, task.id(), query))
                    .ok()
            });
            Ok(read_text(
                task.id(),
                app.journal.get(task.id()).as_ref(),
                query,
                indexed.as_deref(),
            ))
        }
        "brief" => {
            let task = named_task(app, agent, input)?;
            Ok(brief_text(&task))
        }
        other => Err(format!("{other} is not a journal tool")),
    }
}

/// Writes a compaction's summary (the conversation's decisions, dead ends, open questions and where it stood) into the
/// journal of the task session `cid` works on, as a handover of that session. A session with no task has no journal,
/// so nothing is written for it; the summary stays in its transcript.
pub fn record_compaction(app: &App, cid: u64, summary: &str) -> Option<u64> {
    let session = app.sessions.by_cid(cid)?;
    let task = session.task.clone()?;
    Some(app.journal.add_handover(
        &task,
        compaction_handover(&session.id, summary, apz_plaza::now_ms()),
    ))
}

/// The handover a compaction leaves: the summary is where things stand, and the session goes on from it.
pub fn compaction_handover(session: &str, summary: &str, at_ms: u64) -> Handover {
    Handover {
        seq: 0,
        session: session.to_string(),
        stands: summary.trim().to_string(),
        done: String::new(),
        next: "The conversation before this summary was compacted: continue from it.".to_string(),
        check_first: String::new(),
        at_ms,
    }
}

/// The session a write comes from and its task: only a task's session writes its journal.
fn writing(app: &App, agent: &Agent) -> Result<(crate::sessions::Session, String), String> {
    let session = app
        .sessions
        .by_cid(agent.cid)
        .ok_or_else(|| "only a session working on a task writes its journal".to_string())?;
    let task = session
        .task
        .clone()
        .ok_or_else(|| "this session has no task, so it has no journal to write".to_string())?;
    Ok((session, task))
}

/// The task a reader is about: the one the input names, which must be in the agent's project, else the agent's own.
fn named_task(
    app: &App,
    agent: &Agent,
    input: &Value,
) -> Result<std::sync::Arc<apz_plaza::tasks::Task>, String> {
    let id = match input.get("task").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => app
            .sessions
            .by_cid(agent.cid)
            .and_then(|s| s.task)
            .ok_or_else(|| "name the task (task): this session has none".to_string())?,
    };
    let task = app
        .plaza
        .tasks
        .find(&id)
        .ok_or_else(|| format!("no task {id}"))?;
    if task.project() != crate::projects::project_of(app, agent.cid).as_deref() {
        return Err(format!("task {id} is not in your project"));
    }
    Ok(task)
}

/// A note as `journal_note`'s input says it. A decision needs its `reason`, a dead end its `why`, an open question
/// may name its `thread`.
fn note_from_input(input: &Value) -> Result<Note, String> {
    let kind = match input.get("kind").and_then(Value::as_str) {
        Some("decision") => Kind::Decision,
        Some("dead_end") => Kind::DeadEnd,
        Some("open_question") => Kind::OpenQuestion,
        _ => {
            return Err("invalid input: kind is decision, dead_end or open_question".to_string());
        }
    };
    let reason = match kind {
        Kind::Decision => Some(str_arg(input, "reason")?),
        Kind::DeadEnd => Some(str_arg(input, "why")?),
        Kind::OpenQuestion => None,
    };
    let thread = match kind {
        Kind::OpenQuestion => input
            .get("thread")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    };
    Ok(Note {
        seq: 0,
        kind,
        session: None,
        text: str_arg(input, "text")?,
        reason,
        thread,
        at_ms: 0,
    })
}

/// A handover as `handover_write`'s input says it: `stands` and `next` are required, `done` and `check_first` are not.
fn handover_from_input(input: &Value) -> Result<Handover, String> {
    let optional = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    Ok(Handover {
        seq: 0,
        session: String::new(),
        stands: str_arg(input, "stands")?,
        done: optional("done"),
        next: str_arg(input, "next")?,
        check_first: optional("check_first"),
        at_ms: 0,
    })
}

/// A required, non-empty string argument.
fn str_arg(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing or empty string `{key}`"))
}

/// A handover as the next session reads it: where things stand, then what is done, next and to check first.
pub fn handover_text(handover: &Handover) -> String {
    let mut out = format!("Where things stand: {}\n", handover.stands);
    if !handover.done.is_empty() {
        out.push_str(&format!("Done: {}\n", handover.done));
    }
    out.push_str(&format!("Next: {}\n", handover.next));
    if !handover.check_first.is_empty() {
        out.push_str(&format!("Check first: {}\n", handover.check_first));
    }
    out
}

/// A task's brief: its goal, its spec and acceptance, and the tasks it runs after.
pub fn brief_text(task: &apz_plaza::tasks::Task) -> String {
    let mut out = format!("Task {}: {}\n", task.id(), task.goal());
    if let Some(acceptance) = task.acceptance() {
        out.push_str(&format!("Spec and acceptance:\n{acceptance}\n"));
    }
    let after = task.after();
    if !after.is_empty() {
        out.push_str(&format!("Runs after: {}\n", after.join(", ")));
    }
    out
}

/// The notes one per line, as the prompt and `journal_read` show them: a decision's reason and a dead end's why follow
/// it, an open question shows its thread.
pub fn notes_text(notes: &[Note]) -> String {
    let mut out = String::new();
    for note in notes {
        out.push_str(&format!(
            "- {} n{}: {}",
            note.kind.label(),
            note.seq,
            note.text
        ));
        match (note.kind, &note.reason) {
            (Kind::Decision, Some(reason)) => out.push_str(&format!(" Reason: {reason}")),
            (Kind::DeadEnd, Some(why)) => out.push_str(&format!(" Why: {why}")),
            _ => {}
        }
        if let Some(thread) = &note.thread {
            out.push_str(&format!(" Thread: {thread}"));
        }
        out.push('\n');
    }
    out
}

/// What `journal_read` returns: the notes (those matching `query`, when one is given) and the latest handover. The
/// index's answer comes as `indexed`, the matching notes' numbers best match first; without it, `query` is matched
/// in memory.
pub fn read_text(
    task: &str,
    journal: Option<&Journal>,
    query: Option<&str>,
    indexed: Option<&[u64]>,
) -> String {
    let Some(journal) = journal else {
        return format!("The journal of {task} is empty.");
    };
    let notes: Vec<Note> = match (query, indexed) {
        (Some(_), Some(seqs)) => seqs
            .iter()
            .filter_map(|seq| journal.notes.iter().find(|note| note.seq == *seq))
            .cloned()
            .collect(),
        (Some(q), None) => journal
            .notes
            .iter()
            .filter(|note| matches(note, q))
            .cloned()
            .collect(),
        (None, _) => journal.notes.clone(),
    };
    let mut out = if notes.is_empty() {
        format!("No notes on {task} match.\n")
    } else {
        notes_text(&notes)
    };
    if let Some(handover) = journal.handovers.last() {
        out.push_str(&format!(
            "Latest handover (from session {}):\n{}",
            handover.session,
            handover_text(handover)
        ));
    }
    out
}

/// Whether a note's text or reason holds `query`, ignoring case.
fn matches(note: &Note, query: &str) -> bool {
    let query = query.to_lowercase();
    note.text.to_lowercase().contains(&query)
        || note
            .reason
            .as_deref()
            .is_some_and(|reason| reason.to_lowercase().contains(&query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note(text: &str, kind: Kind, reason: Option<&str>) -> Note {
        Note {
            seq: 0,
            kind,
            session: Some("s1".into()),
            text: text.into(),
            reason: reason.map(str::to_string),
            thread: None,
            at_ms: 1,
        }
    }

    fn handover(stands: &str, next: &str) -> Handover {
        Handover {
            seq: 0,
            session: "s1".into(),
            stands: stands.into(),
            done: String::new(),
            next: next.into(),
            check_first: String::new(),
            at_ms: 1,
        }
    }

    #[test]
    fn notes_and_handovers_are_numbered_per_task_and_kept_apart() {
        let journals = Journals::default();
        assert_eq!(
            journals.add_note("t1", note("a", Kind::Decision, Some("why"))),
            1
        );
        assert_eq!(
            journals.add_note("t1", note("b", Kind::DeadEnd, Some("broke"))),
            2
        );
        assert_eq!(
            journals.add_note("t2", note("c", Kind::OpenQuestion, None)),
            1
        );
        assert_eq!(journals.add_handover("t1", handover("half", "finish")), 1);
        let t1 = journals.get("t1").unwrap();
        let texts: Vec<(u64, &str)> = t1.notes.iter().map(|n| (n.seq, n.text.as_str())).collect();
        assert_eq!(texts, [(1, "a"), (2, "b")]);
        assert_eq!(t1.handovers.len(), 1);
        assert_eq!(journals.get("t2").unwrap().notes.len(), 1);
        assert!(journals.get("t3").is_none());
    }

    #[test]
    fn a_decision_needs_its_reason_a_dead_end_its_why_and_an_open_question_may_name_a_thread() {
        let decision = note_from_input(
            &json!({"kind": "decision", "text": "use sqlite", "reason": "one file"}),
        )
        .unwrap();
        assert_eq!(
            (decision.kind, decision.reason.as_deref()),
            (Kind::Decision, Some("one file"))
        );
        let refused =
            note_from_input(&json!({"kind": "decision", "text": "use sqlite"})).unwrap_err();
        assert_eq!(refused, "missing or empty string `reason`");
        let dead =
            note_from_input(&json!({"kind": "dead_end", "text": "tried pools", "why": "locks"}))
                .unwrap();
        assert_eq!(dead.reason.as_deref(), Some("locks"));
        assert!(
            note_from_input(&json!({"kind": "dead_end", "text": "tried pools", "reason": "x"}))
                .is_err()
        );
        let open =
            note_from_input(&json!({"kind": "open_question", "text": "retry?", "thread": "th4"}))
                .unwrap();
        assert_eq!(
            (open.kind, open.reason, open.thread.as_deref()),
            (Kind::OpenQuestion, None, Some("th4"))
        );
        let other = note_from_input(&json!({"kind": "guess", "text": "x"})).unwrap_err();
        assert!(
            other.contains("decision, dead_end or open_question"),
            "{other}"
        );
    }

    #[test]
    fn a_handover_needs_where_things_stand_and_what_is_next() {
        let full = handover_from_input(&json!({
            "stands": "parser done", "done": "tests", "next": "wire it", "check_first": "the lock"
        }))
        .unwrap();
        assert_eq!(
            handover_text(&full),
            "Where things stand: parser done\nDone: tests\nNext: wire it\nCheck first: the lock\n"
        );
        let bare = handover_from_input(&json!({"stands": "half", "next": "finish"})).unwrap();
        assert_eq!(
            handover_text(&bare),
            "Where things stand: half\nNext: finish\n"
        );
        assert!(handover_from_input(&json!({"stands": "half"})).is_err());
    }

    #[test]
    fn the_journal_reads_back_its_notes_by_text_and_reason_and_its_latest_handover() {
        let journals = Journals::default();
        journals.add_note(
            "t1",
            note("retry the upload", Kind::Decision, Some("the lock is slow")),
        );
        journals.add_note("t1", note("pools", Kind::DeadEnd, Some("the mover stalls")));
        journals.add_handover("t1", handover("first", "second"));
        journals.add_handover("t1", handover("latest", "last"));
        let journal = journals.get("t1");
        let all = read_text("t1", journal.as_ref(), None, None);
        assert!(
            all.contains("- decision n1: retry the upload Reason: the lock is slow\n"),
            "{all}"
        );
        assert!(
            all.contains("- dead end n2: pools Why: the mover stalls\n"),
            "{all}"
        );
        assert!(
            all.ends_with(
                "Latest handover (from session s1):\nWhere things stand: latest\nNext: last\n"
            ),
            "{all}"
        );
        let by_reason = read_text("t1", journal.as_ref(), Some("LOCK"), None);
        assert!(
            by_reason.contains("n1") && !by_reason.contains("n2"),
            "{by_reason}"
        );
        assert!(
            read_text("t1", journal.as_ref(), Some("nothing"), None)
                .starts_with("No notes on t1 match.")
        );
        // The index's answer is listed in its order, best match first.
        let ranked = read_text("t1", journal.as_ref(), Some("lock"), Some(&[2, 1]));
        assert!(
            ranked.find("n2").unwrap() < ranked.find("n1").unwrap(),
            "{ranked}"
        );
        assert_eq!(
            read_text("t9", None, None, None),
            "The journal of t9 is empty."
        );
    }

    #[test]
    fn a_compaction_leaves_its_summary_as_the_latest_handover_of_the_task() {
        let journals = Journals::default();
        journals.add_handover("t1", handover("parser done", "wire it"));
        let summary = "Decisions: sessions, not agents (reason: a seat is a session).\nWhere it stands: parser done.";
        journals.add_handover("t1", compaction_handover("s7", summary, 9));
        let journal = journals.get("t1").expect("t1's journal");
        let latest = journal.handovers.last().expect("a handover");
        assert_eq!(
            (latest.seq, latest.session.as_str()),
            (2, "s7"),
            "{latest:?}"
        );
        assert_eq!(latest.stands, summary, "{latest:?}");
        assert!(
            read_text("t1", Some(&journal), None, None).contains("Latest handover (from session s7):\nWhere things stand: Decisions: sessions, not agents"),
            "{}",
            read_text("t1", Some(&journal), None, None)
        );
    }

    #[test]
    fn the_journal_tools_are_host_tools_every_role_is_offered() {
        let offered = crate::tools::offered_names("builder");
        for name in TOOLS {
            assert_eq!(
                crate::tools::runs(name),
                Some(crate::tools::Runs::Host),
                "{name}"
            );
            assert!(
                offered.iter().any(|offer| offer == name),
                "{name} is not offered"
            );
        }
    }
}
