//! Sessions (PLAN 4.3, 5.1): the work engine's unit of work. A session is one fresh context working one recipe step
//! of one task (a `step` session), a project's planner or the main orchestrator (`long`, compacting as it goes), or a
//! helper a session started (`helper`). It runs in a fork: a VM of its own, started for it (from today's templates
//! until phase 3's bases), whose CID is the session's while it lives. A session has no name and no memory
//! of its own: what it learns goes into its task's journal and, through the curator, into project memory.
//!
//! The records live in memory, indexed by id and by CID; the hub's saver keeps them in `plaza.db` (table `session`).
//!
//! ```text
//! queued ─► placing ─► forking ─► running ─► handing_over ─► ending ─► ended
//!    │         │          │        │  ▲  ▲
//!    │         │          │        ▼  │  └── compacting (long)
//!    │         │          │      paused (budget · allowance · disk · card · node)
//!    └─────────┴──────────┴──► failed
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// What a session is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Works one step of a task and ends with it (with a handover).
    Step,
    /// Long-lived: a project's planner or the main orchestrator; compacts instead of ending.
    Long,
    /// Started by another session for a bounded job in its fork (W8).
    Helper,
}

/// Where a session is in its life (5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Placing,
    Forking,
    Running,
    Paused,
    Compacting,
    HandingOver,
    Ending,
    Ended,
    Failed,
}

impl State {
    /// Whether the session is over.
    pub fn is_final(self) -> bool {
        matches!(self, State::Ended | State::Failed)
    }

    /// Whether a session may move from `self` to `to`.
    pub fn can_move(self, to: State) -> bool {
        use State::*;
        match (self, to) {
            (from, Failed) => !from.is_final(),
            (Queued, Placing) | (Placing, Forking) | (Forking, Running) => true,
            (Running, Paused | Compacting | HandingOver | Ending) => true,
            (Paused, Running | HandingOver | Ending) => true,
            (Compacting, Running | Paused) => true,
            (HandingOver, Ending) | (Ending, Ended) => true,
            // A session that never ran can end at once (stopped while it queued or placed).
            (Queued | Placing | Forking, Ending | Ended) => true,
            _ => false,
        }
    }
}

/// Why a session is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum PausedBy {
    Budget,
    Allowance,
    Disk,
    Card,
    Node,
}

/// What a session has used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Usage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cached: u64,
    /// Micro-dollars.
    pub cost_micros: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
}

impl Usage {
    fn add(&mut self, other: &Usage) {
        self.tokens_in += other.tokens_in;
        self.tokens_out += other.tokens_out;
        self.tokens_cached += other.tokens_cached;
        self.cost_micros += other.cost_micros;
        self.model_calls += other.model_calls;
        self.tool_calls += other.tool_calls;
    }
}

/// The outcome of a session that was live when the hub stopped (its VM did not survive the hub).
pub const RESTARTED: &str = "the hub restarted";

/// One session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "session")]
pub struct Session {
    /// `s<seq>`.
    #[table(key)]
    pub id: String,
    /// The project it works for (the built-in project `plaza` for the main orchestrator).
    #[table(index)]
    pub project: String,
    /// The task it works on; none for a long-lived session.
    #[table(index)]
    pub task: Option<String>,
    /// The seat it fills in its task (`builder`, `reviewer:1`, …).
    pub seat: Option<String>,
    pub kind: Kind,
    /// For a helper, the session that started it.
    pub parent: Option<String>,
    /// The seat template (role) it runs as.
    pub role: String,
    /// `harness` (our own loop) or the id of a vendor engine over ACP.
    pub engine: String,
    pub model: Option<String>,
    pub state: State,
    pub paused_by: Option<PausedBy>,
    /// Its fork: the node, the VM's CID and noded's run.
    pub node: Option<String>,
    #[table(index)]
    pub cid: Option<u64>,
    pub run: Option<String>,
    /// The work item in flight on its CID's lane.
    pub work: Option<u64>,
    #[table(flatten)]
    pub usage: Usage,
    pub context_bytes: u64,
    pub compactions: u32,
    /// What it handed over to the next session of its seat when it ended (kept as written).
    #[table(content)]
    pub handover: Option<String>,
    /// Why it ended or failed, in a sentence.
    pub outcome: Option<String>,
    /// Which try of its seat this is (1, 2, …).
    pub attempt: u32,
    /// A reviewer's verdict, once given (true: passed).
    pub verdict: Option<bool>,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

/// What a new session needs.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub project: String,
    pub task: Option<String>,
    pub seat: Option<String>,
    pub kind: Kind,
    pub parent: Option<String>,
    pub role: String,
    pub engine: String,
    pub model: Option<String>,
    pub attempt: u32,
}

/// Which sessions to list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub project: Option<String>,
    pub task: Option<String>,
    pub state: Option<State>,
    /// Only sessions not yet ended or failed.
    pub live: bool,
}

#[derive(Default)]
struct Inner {
    next: u64,
    by_id: BTreeMap<String, Session>,
    by_cid: HashMap<u64, String>,
}

/// Every session.
#[derive(Default)]
pub struct Sessions {
    inner: Mutex<Inner>,
}

/// A refused change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotFound(String),
    Move { id: String, from: State, to: State },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotFound(id) => write!(f, "no session {id}"),
            Refused::Move { id, from, to } => {
                write!(f, "session {id} cannot move from {from:?} to {to:?}")
            }
        }
    }
}

fn seq_of(id: &str) -> Option<u64> {
    id.strip_prefix('s')?.parse().ok()
}

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records a new session, queued.
    pub fn create(&self, new: NewSession, now_ms: u64) -> Session {
        let mut inner = self.lock();
        inner.next += 1;
        let session = Session {
            id: format!("s{}", inner.next),
            project: new.project,
            task: new.task,
            seat: new.seat,
            kind: new.kind,
            parent: new.parent,
            role: new.role,
            engine: new.engine,
            model: new.model,
            state: State::Queued,
            paused_by: None,
            node: None,
            cid: None,
            run: None,
            work: None,
            usage: Usage::default(),
            context_bytes: 0,
            compactions: 0,
            handover: None,
            outcome: None,
            attempt: new.attempt.max(1),
            verdict: None,
            created_ms: now_ms,
            started_ms: None,
            ended_ms: None,
        };
        inner.by_id.insert(session.id.clone(), session.clone());
        session
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        self.lock().by_id.get(id).cloned()
    }

    /// The live session whose fork is VM `cid`.
    pub fn by_cid(&self, cid: u64) -> Option<Session> {
        let inner = self.lock();
        inner
            .by_cid
            .get(&cid)
            .and_then(|id| inner.by_id.get(id))
            .cloned()
    }

    /// The sessions `filter` selects, newest first.
    pub fn list(&self, filter: &Filter) -> Vec<Session> {
        let inner = self.lock();
        let mut out: Vec<Session> = inner
            .by_id
            .values()
            .filter(|s| filter.project.as_ref().is_none_or(|p| &s.project == p))
            .filter(|s| {
                filter
                    .task
                    .as_ref()
                    .is_none_or(|t| s.task.as_ref() == Some(t))
            })
            .filter(|s| filter.state.is_none_or(|st| s.state == st))
            .filter(|s| !filter.live || !s.state.is_final())
            .cloned()
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(seq_of(&s.id).unwrap_or(0)));
        out
    }

    /// Changes a session with `f`, keeping the CID index true. Returns the session as changed.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut Session)) -> Result<Session, Refused> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let session = inner
            .by_id
            .get_mut(id)
            .ok_or_else(|| Refused::NotFound(id.to_string()))?;
        let before = session.cid;
        f(session);
        let after = if session.state.is_final() {
            None
        } else {
            session.cid
        };
        let id = session.id.clone();
        let changed = session.clone();
        if before != after {
            if let Some(cid) = before
                && inner.by_cid.get(&cid) == Some(&id)
            {
                inner.by_cid.remove(&cid);
            }
            if let Some(cid) = after {
                inner.by_cid.insert(cid, id);
            }
        }
        Ok(changed)
    }

    /// Moves a session to `to` (refused when 5.1 does not allow it), stamping when it started or ended.
    pub fn set_state(&self, id: &str, to: State, now_ms: u64) -> Result<Session, Refused> {
        let current = self
            .get(id)
            .ok_or_else(|| Refused::NotFound(id.to_string()))?;
        if current.state == to {
            return Ok(current);
        }
        if !current.state.can_move(to) {
            return Err(Refused::Move {
                id: id.to_string(),
                from: current.state,
                to,
            });
        }
        self.update(id, |s| {
            s.state = to;
            if to != State::Paused {
                s.paused_by = None;
            }
            if to == State::Running && s.started_ms.is_none() {
                s.started_ms = Some(now_ms);
            }
            if to.is_final() {
                s.ended_ms = Some(now_ms);
                s.work = None;
            }
        })
    }

    /// Pauses a running (or compacting) session for `why`.
    pub fn pause(&self, id: &str, why: PausedBy, now_ms: u64) -> Result<Session, Refused> {
        self.set_state(id, State::Paused, now_ms)?;
        self.update(id, |s| s.paused_by = Some(why))
    }

    /// Ends a session (from any live state) with its outcome; `failed` marks it failed.
    pub fn finish(
        &self,
        id: &str,
        outcome: &str,
        failed: bool,
        now_ms: u64,
    ) -> Result<Session, Refused> {
        let to = if failed { State::Failed } else { State::Ended };
        let current = self
            .get(id)
            .ok_or_else(|| Refused::NotFound(id.to_string()))?;
        if current.state.is_final() {
            return Ok(current);
        }
        self.update(id, |s| {
            s.state = to;
            s.outcome = Some(outcome.to_string());
            s.ended_ms = Some(now_ms);
            s.work = None;
            s.paused_by = None;
        })
    }

    /// Adds `usage` to the session of VM `cid`, if one lives there. Returns the session's id.
    pub fn add_usage(&self, cid: u64, usage: &Usage) -> Option<String> {
        let mut inner = self.lock();
        let id = inner.by_cid.get(&cid).cloned()?;
        inner.by_id.get_mut(&id)?.usage.add(usage);
        Some(id)
    }

    /// The sessions' rows, for the hub's store.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        apz_store::RowSet::list(def(), inner.by_id.values())
    }

    /// Replaces the sessions with what the store holds (at start). Sessions that were live when the hub stopped are
    /// failed: their forks did not survive the hub (no respawn at boot; the seats they filled start again).
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Session> = apz_store::table::load_list(conn, def())?;
        let now = apz_plaza::now_ms();
        let mut inner = Inner::default();
        for mut s in stored {
            inner.next = inner.next.max(seq_of(&s.id).unwrap_or(0));
            if !s.state.is_final() && s.kind != Kind::Long {
                s.state = State::Failed;
                s.outcome = Some(RESTARTED.into());
                s.ended_ms = Some(now);
                s.work = None;
            }
            if !s.state.is_final()
                && let Some(cid) = s.cid
            {
                inner.by_cid.insert(cid, s.id.clone());
            }
            inner.by_id.insert(s.id.clone(), s);
        }
        *self.lock() = inner;
        Ok(())
    }
}

/// The sessions' table.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(task: &str) -> NewSession {
        NewSession {
            project: "pr1".into(),
            task: Some(task.into()),
            seat: Some("builder".into()),
            kind: Kind::Step,
            parent: None,
            role: "builder".into(),
            engine: "harness".into(),
            model: Some("anthropic/claude-x".into()),
            attempt: 1,
        }
    }

    #[test]
    fn a_session_moves_through_its_life_and_its_cid_is_indexed_while_it_lives() {
        let sessions = Sessions::default();
        let s = sessions.create(step("t1"), 1);
        assert_eq!((s.id.as_str(), s.state), ("s1", State::Queued));
        sessions.set_state("s1", State::Placing, 2).unwrap();
        sessions.set_state("s1", State::Forking, 3).unwrap();
        sessions.update("s1", |s| s.cid = Some(300)).unwrap();
        assert_eq!(sessions.by_cid(300).unwrap().id, "s1");
        let running = sessions.set_state("s1", State::Running, 4).unwrap();
        assert_eq!(running.started_ms, Some(4));
        assert!(
            sessions.set_state("s1", State::Queued, 5).is_err(),
            "no way back to queued"
        );
        sessions.add_usage(
            300,
            &Usage {
                tokens_in: 10,
                model_calls: 1,
                ..Usage::default()
            },
        );
        sessions.set_state("s1", State::HandingOver, 6).unwrap();
        sessions.set_state("s1", State::Ending, 7).unwrap();
        let ended = sessions.set_state("s1", State::Ended, 8).unwrap();
        assert_eq!((ended.usage.tokens_in, ended.ended_ms), (10, Some(8)));
        assert!(
            sessions.by_cid(300).is_none(),
            "an ended session leaves the CID index"
        );
    }

    #[test]
    fn a_paused_session_resumes_and_a_failed_one_is_final() {
        let sessions = Sessions::default();
        sessions.create(step("t1"), 1);
        for to in [State::Placing, State::Forking, State::Running] {
            sessions.set_state("s1", to, 2).unwrap();
        }
        let paused = sessions.pause("s1", PausedBy::Budget, 3).unwrap();
        assert_eq!(paused.paused_by, Some(PausedBy::Budget));
        let resumed = sessions.set_state("s1", State::Running, 4).unwrap();
        assert_eq!(resumed.paused_by, None);
        sessions.finish("s1", "no node fits", true, 5).unwrap();
        assert!(sessions.set_state("s1", State::Running, 6).is_err());
    }

    #[test]
    fn sessions_round_trip_through_the_store_and_live_steps_fail_on_load() {
        let sessions = Sessions::default();
        sessions.create(step("t1"), 1);
        sessions.create(step("t2"), 2);
        sessions.set_state("s2", State::Placing, 3).unwrap();
        sessions.finish("s1", "done", false, 4).unwrap();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &sessions.rows().unwrap()).unwrap();
        let loaded = Sessions::default();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.get("s1"), sessions.get("s1"));
        let s2 = loaded.get("s2").unwrap();
        assert_eq!(
            (s2.state, s2.outcome.as_deref()),
            (State::Failed, Some(RESTARTED))
        );
        assert_eq!(
            loaded.create(step("t3"), 5).id,
            "s3",
            "ids continue after the stored ones"
        );
        let live = loaded.list(&Filter {
            live: true,
            ..Filter::default()
        });
        assert_eq!(live.len(), 1);
    }
}
