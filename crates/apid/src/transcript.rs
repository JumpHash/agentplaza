//! Per-agent transcripts: everything an agent's loop did, in order.
//!
//! An entry is a typed [`Entry`], one variant per kind. What the hub reads back (counts, ids, names, statuses,
//! flags) is a typed field; what a model, a tool, an engine or a person wrote is kept as written, in a
//! `#[table(content)]` field. The store keeps each entry as one row of `entry` in `transcripts.db`
//! (`sql/transcripts/0001_init.sql`); the window keeps the newest entries of every agent in memory, and the live
//! reads (the API, the feeds, the search) use it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use apz_store::rusqlite::Connection;
use apz_store::table::{Changes, Loaded, Row, Value as SqlValue};
use apz_store::{Db, IndexDef, RowSet, StoreError, Table as _, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::Bus;
use crate::store::sql::{TRANSCRIPT_CIDS, TRANSCRIPT_DELETE, TRANSCRIPT_PAGE};

/// Entries kept per agent; older ones are dropped.
const KEEP: usize = 5_000;

/// Bytes kept per agent (each entry as JSON); older ones are dropped past this. The newest entry always stays, even
/// when it alone is over the budget.
const MAX_BYTES_PER_AGENT: usize = 1 << 20;

/// The newest entries of each agent that are read back from the store at start.
pub const RESTORE_PER_AGENT: usize = 500;

/// How a work item ended (`work_end`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Stopped,
}

/// What a work item is about (`work_start`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Task,
    Chat,
    Message,
}

impl From<apz_proto::wire::WorkKind> for WorkKind {
    fn from(kind: apz_proto::wire::WorkKind) -> Self {
        match kind {
            apz_proto::wire::WorkKind::Task => WorkKind::Task,
            apz_proto::wire::WorkKind::Chat => WorkKind::Chat,
            apz_proto::wire::WorkKind::Message => WorkKind::Message,
        }
    }
}

/// The verdict on a tool request (`tool_decision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Deny,
    Done,
    Pending,
}

impl From<apz_proto::wire::Decision> for Verdict {
    fn from(decision: apz_proto::wire::Decision) -> Self {
        match decision {
            apz_proto::wire::Decision::Allow => Verdict::Allow,
            apz_proto::wire::Decision::Deny => Verdict::Deny,
            apz_proto::wire::Decision::Done => Verdict::Done,
            apz_proto::wire::Decision::Pending => Verdict::Pending,
        }
    }
}

/// The tokens of one model call (`model_response`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

impl From<&apz_proto::chat::Usage> for Usage {
    fn from(usage: &apz_proto::chat::Usage) -> Self {
        Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }
    }
}

/// What every sandbox event carries (the `sandbox_*` kinds): the event, the sandbox, its agent, its template, and
/// who acted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct SandboxBase {
    pub event: String,
    pub sandbox: String,
    pub cid: u64,
    pub template: String,
    pub by: String,
}

/// One transcript entry, by kind. The serialized form is `{"kind": "...", "data": {...}}`, the same the consoles read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Entry {
    /// A work item started: its kind, its thread, its prompt, and where it came from (`origin`, as it is).
    WorkStart {
        kind: WorkKind,
        thread: String,
        #[table(content)]
        prompt: String,
        #[table(content)]
        origin: Value,
    },
    /// A work item ended, with its outcome and its summary.
    WorkEnd {
        status: Outcome,
        #[table(content)]
        summary: String,
    },
    /// A work item was interrupted to resume on another machine.
    WorkInterrupted { reason: String },
    /// A model request as the hub summarized it: the model, the number of messages, the tool names, and the last
    /// user turn (clipped by the caller).
    ModelRequest {
        model: String,
        messages: usize,
        #[table(content)]
        tools: Value,
        #[table(content)]
        last_user: String,
    },
    /// A model's answer. The harness records its whole answer (`model`, `content`, `stop`, `usage`); the gateway
    /// records a call the harness did not make (`via`, `model`, `usage`, `cost_usd`).
    ModelResponse {
        #[serde(skip_serializing_if = "Option::is_none")]
        via: Option<String>,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[table(content)]
        content: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[table(content)]
        stop: Option<Value>,
        #[table(flatten)]
        usage: Usage,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
    /// A model call the task's budget could not cover in full.
    ModelChargeShortfall {
        model: String,
        task: Option<String>,
        wanted: i64,
        charged: i64,
        available: i64,
        error: Option<String>,
    },
    /// A tool request from the model: its call id, the tool's name, and its input as the model sent it.
    ToolRequest {
        call: String,
        name: String,
        #[table(content)]
        input: Value,
    },
    /// The verdict on a tool request, and the output the decision produced (capped by the caller).
    ToolDecision {
        call: String,
        decision: Verdict,
        #[table(content)]
        output: String,
    },
    /// A tool's report: whether it worked, and its output (capped by the caller).
    ToolResult {
        call: String,
        ok: bool,
        #[table(content)]
        output: String,
    },
    /// One item of an ACP engine's session feed (`{call, item, ...}`), as the feed keeps it.
    EngineUpdate(#[table(content)] Value),
    /// An error the hub recorded for the agent.
    Error { message: String },
    /// The agent's context was compressed: its size before and after, the turns folded, and why.
    Compressed {
        before: u64,
        after: u64,
        folded: u32,
        reason: String,
    },
    /// An egress decision of the agent's VM, as the egress proxy reported it.
    Egress {
        host: String,
        port: u16,
        allowed: bool,
        bytes_up: u64,
        bytes_down: u64,
        ms: u64,
    },
    /// An egress request that had a credential injected: the host, the method, the path (before any query), the rule.
    EgressInject {
        host: String,
        method: String,
        path: String,
        rule: String,
    },
    /// An MCP tool call of the agent: the server and tool, how long it took, whether it worked, the size of its output.
    McpCall {
        server: String,
        tool: String,
        ms: u64,
        ok: bool,
        output_bytes: usize,
    },
    /// An artifact the agent stored: its id, version, title and kind.
    Artifact {
        id: String,
        version: u32,
        title: String,
        kind: String,
    },
    /// A decision the hub took with a model for the agent: the provider and model, the questions, the input tokens.
    Decision {
        provider: String,
        model: String,
        questions: usize,
        input_tokens: u64,
    },
    /// A control sent to the agent's running work (`{"action": ...}` with its fields).
    Control {
        #[table(content)]
        control: Value,
    },
    /// A VM checkpoint: the checkpoint itself and whether it was automatic, or the name of a deleted one.
    VmCheckpoint {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[table(content)]
        checkpoint: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        auto: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        deleted: Option<String>,
    },
    /// The VM was restored to a checkpoint, after a safety checkpoint of its own state.
    VmRestore {
        name: String,
        #[table(content)]
        safety: Value,
    },
    /// The agent is a fork of `cid`, made from `checkpoint`.
    ForkedFrom { cid: u64, checkpoint: String },
    /// The agent was forked: its children (`{cid, run}` each) and the checkpoint they came from.
    Fork {
        #[table(content)]
        forks: Value,
        checkpoint: String,
    },
    /// The agent started moving between nodes.
    MoveStarted { from: String, to: String },
    /// The agent moved: its size and how long the move took.
    Moved {
        from: String,
        to: String,
        bytes: u64,
        ms: u64,
    },
    /// What the plaza paid a worker for a task, and what the task has spent.
    Payment {
        task: String,
        amount: i64,
        spent: i64,
    },
    /// The agent woke from sleep.
    Wake {},
    /// The agent went to sleep, with the snapshot noded reported (null when none).
    Sleep {
        #[table(content)]
        snapshot: Value,
    },
    /// The agent asked the owner to share an artifact: its route id, its title, and the approval that holds it.
    ShareRequest {
        artifact: String,
        title: String,
        approval: String,
    },
    /// The owner decided on a share request.
    ShareDecision {
        #[table(content)]
        text: String,
        approved: bool,
        title: String,
    },
    /// A sandbox was created for the agent.
    SandboxCreate {
        #[serde(flatten)]
        #[table(flatten)]
        base: SandboxBase,
        timeout: u32,
        allow_internet_access: bool,
    },
    /// A sandbox was killed; `kept_for_snapshots` counts the snapshots that outlive it.
    SandboxKill {
        #[serde(flatten)]
        #[table(flatten)]
        base: SandboxBase,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        kept_for_snapshots: Option<usize>,
    },
    /// A sandbox was paused.
    SandboxPause {
        #[serde(flatten)]
        #[table(flatten)]
        base: SandboxBase,
        reason: String,
    },
    /// A sandbox was resumed.
    SandboxResume {
        #[serde(flatten)]
        #[table(flatten)]
        base: SandboxBase,
        timeout: u32,
    },
    /// A sandbox snapshot was taken or deleted (the event says which).
    SandboxSnapshot {
        #[serde(flatten)]
        #[table(flatten)]
        base: SandboxBase,
        snapshot: String,
    },
    /// A sandbox's network rules changed, or a change is waiting for the owner (`network_pending`, with `why`).
    SandboxNetwork {
        #[serde(flatten)]
        #[table(flatten)]
        base: SandboxBase,
        #[serde(rename = "allowOut")]
        #[table(content)]
        allow_out: Value,
        #[serde(rename = "denyOut")]
        #[table(content)]
        deny_out: Value,
        allow_internet_access: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
}

/// One stored entry: the agent it belongs to, its sequence number (the order of the whole store, unique per
/// entry), the session it belongs to (none yet), the work item it ran under (0 for none), and when it was recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "entry")]
pub struct StoredEntry {
    #[table(key)]
    pub cid: u64,
    #[table(key)]
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub work: u64,
    pub at_ms: u64,
    #[serde(flatten)]
    #[table(flatten)]
    pub entry: Entry,
}

/// The table `entry` in `transcripts.db`: keyed by (cid, seq), with an index on (session, seq).
pub fn def() -> &'static TableDef {
    static DEF: OnceLock<TableDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = StoredEntry::root();
        def.indexes.push(IndexDef {
            columns: vec!["session".to_string(), "seq".to_string()],
            unique: false,
        });
        def
    })
}

/// One agent's window of recent entries, oldest first. Each entry's size is computed once on insert and kept beside
/// it, so eviction can bring `bytes` back under the budget.
#[derive(Default)]
struct Window {
    entries: VecDeque<StoredEntry>,
    sizes: VecDeque<usize>,
    bytes: usize,
}

impl Window {
    fn push(&mut self, entry: StoredEntry, size: usize) {
        // Kept sorted by seq so `after_seq` can binary search. A record takes its seq before it takes the
        // lock, so two records racing on one agent can arrive out of order; they are slotted in place here.
        let at = self.entries.partition_point(|e| e.seq < entry.seq);
        self.entries.insert(at, entry);
        self.sizes.insert(at, size);
        self.bytes += size;
        while self.entries.len() > 1
            && (self.entries.len() > KEEP || self.bytes > MAX_BYTES_PER_AGENT)
        {
            self.entries.pop_front();
            if let Some(size) = self.sizes.pop_front() {
                self.bytes -= size;
            }
        }
    }

    /// The entries with `seq > after`, oldest first.
    fn after_seq(&self, after: u64) -> impl DoubleEndedIterator<Item = &StoredEntry> {
        let start = self.entries.partition_point(|e| e.seq <= after);
        self.entries.range(start..)
    }

    fn iter(&self) -> impl DoubleEndedIterator<Item = &StoredEntry> {
        self.entries.iter()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The bytes an entry counts for in the window: its JSON.
fn size_of(entry: &Entry) -> usize {
    serde_json::to_vec(entry).map_or(0, |bytes| bytes.len())
}

/// An entry's data (what its kind carries), as JSON: the search looks in it.
fn data_of(entry: &Entry) -> Value {
    serde_json::to_value(entry).map_or(Value::Null, |mut value| value["data"].take())
}

/// The session a VM works in, by its CID (`crate::sessions`), for keying entries by session.
pub type SessionOf = Box<dyn Fn(u64) -> Option<String> + Send + Sync>;

#[derive(Default)]
pub struct Store {
    seq: AtomicU64,
    agents: Mutex<HashMap<u64, Window>>,
    /// The transcripts file, once `attach` has set it: entries are written through it as they are recorded.
    db: OnceLock<Arc<Db>>,
    /// Names the session of a VM's entries, once `key_by_session` has set it.
    session_of: OnceLock<SessionOf>,
}

impl Store {
    fn windows(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Window>> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drops the in-memory log of `cid` (the stored rows stay; see [`Store::wipe`]).
    pub fn clear(&self, cid: u64) {
        self.windows().remove(&cid);
    }

    /// Drops the log of `cid`, in memory and in the store: a wipe.
    pub fn wipe(&self, cid: u64) {
        self.clear(cid);
        if let Some(db) = self.db.get() {
            let sent = db.send(move |conn| {
                conn.prepare_cached(TRANSCRIPT_DELETE)?
                    .execute([to_i64(cid)])?;
                Ok(())
            });
            if let Err(err) = sent {
                eprintln!("apid: transcript of agent {cid} not wiped from the store: {err}");
            }
        }
    }

    /// Writes every entry recorded from now on to `db` (the transcripts file), through its writer.
    pub fn attach(&self, db: Arc<Db>) {
        let _ = self.db.set(db);
    }

    /// Keys every entry recorded from now on by the session its VM works in, as `session_of` names it.
    pub fn key_by_session(&self, session_of: SessionOf) {
        let _ = self.session_of.set(session_of);
    }

    /// Loads stored entries back (oldest first) at start, and moves `seq` past the largest.
    pub fn preload(&self, cid: u64, entries: Vec<StoredEntry>) {
        let largest = entries.iter().map(|e| e.seq).max();
        {
            let mut windows = self.windows();
            let log = windows.entry(cid).or_default();
            for entry in entries {
                let size = size_of(&entry.entry);
                log.push(entry, size);
            }
        }
        if let Some(largest) = largest {
            self.seq.fetch_max(largest, Ordering::SeqCst);
        }
    }

    /// Appends an entry, publishes it as a `transcript` event, and writes it to the store when one is attached.
    pub fn record(&self, bus: &Bus, cid: u64, work: u64, entry: Entry) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let stored = StoredEntry {
            cid,
            seq,
            session: self.session_of.get().and_then(|of| of(cid)),
            work,
            at_ms: apz_plaza::now_ms(),
            entry,
        };
        bus.publish("transcript", &stored);
        if let Some(db) = self.db.get() {
            let row = stored.clone();
            if let Err(err) = db.send(move |conn| insert(conn, &row)) {
                eprintln!("apid: transcript entry {seq} of agent {cid} not stored: {err}");
            }
        }
        let size = size_of(&stored.entry);
        self.windows().entry(cid).or_default().push(stored, size);
        seq
    }

    /// The agent's entries with `seq > after`, oldest first, at most `limit`.
    pub fn after(&self, cid: u64, after: u64, limit: usize) -> Vec<StoredEntry> {
        self.windows()
            .get(&cid)
            .map(|log| log.after_seq(after).take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// The entries of `session` (whose VM is `cid`) with `seq > after`, oldest first, at most `limit`.
    pub fn of_session(
        &self,
        cid: u64,
        session: &str,
        after: u64,
        limit: usize,
    ) -> Vec<StoredEntry> {
        self.windows()
            .get(&cid)
            .map(|log| {
                log.after_seq(after)
                    .filter(|e| e.session.as_deref() == Some(session))
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The agent's entries on `thread` with `seq > after`, oldest first, at most `limit`: those of the work items
    /// whose `work_start` names the thread (a conversation's or an editor session's turns, without the others').
    pub fn on_thread(&self, cid: u64, thread: &str, after: u64, limit: usize) -> Vec<StoredEntry> {
        let windows = self.windows();
        let Some(log) = windows.get(&cid) else {
            return Vec::new();
        };
        let works: HashSet<u64> = log
            .iter()
            .filter(|e| matches!(&e.entry, Entry::WorkStart { thread: t, .. } if t == thread))
            .map(|e| e.work)
            .collect();
        log.iter()
            .filter(|e| e.seq > after && works.contains(&e.work))
            .take(limit)
            .cloned()
            .collect()
    }

    /// The agent's newest `n` entries, oldest first (fewer when the log is shorter).
    pub fn last(&self, cid: u64, n: usize) -> Vec<StoredEntry> {
        self.windows()
            .get(&cid)
            .map(|log| {
                log.iter()
                    .skip(log.len().saturating_sub(n))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The agent's entries whose data mentions `needle` (case-insensitive), newest first, at most `limit`.
    pub fn search(&self, cid: u64, needle: &str, limit: usize) -> Vec<StoredEntry> {
        let needle = needle.to_lowercase();
        self.windows()
            .get(&cid)
            .map(|log| {
                log.iter()
                    .rev()
                    .filter(|e| {
                        data_of(&e.entry)
                            .to_string()
                            .to_lowercase()
                            .contains(&needle)
                    })
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn to_i64<T: TryInto<i64>>(value: T) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// Writes one entry as a row. Its position (the table's first non-key column, `pos`) is its seq, so the table reads in
/// the order the entries were recorded.
fn insert(conn: &Connection, stored: &StoredEntry) -> Result<(), StoreError> {
    let def = def();
    let mut rows = RowSet::new();
    stored.put(def, vec![SqlValue::Integer(to_i64(stored.seq))], &mut rows)?;
    let mut changes = Changes::default();
    for row in rows.tables.remove(&def.name).unwrap_or_default() {
        changes.upserts.push((def.name.clone(), row));
    }
    apz_store::table::apply(conn, def, &changes)
}

/// The agents that have stored entries.
pub fn cids(db: &Db) -> Result<Vec<u64>, StoreError> {
    db.read(|conn| {
        let mut stmt = conn.prepare_cached(TRANSCRIPT_CIDS)?;
        let mut out = Vec::new();
        for cid in stmt.query_map([], |r| r.get::<_, i64>(0))? {
            out.push(u64::try_from(cid?).unwrap_or(0));
        }
        Ok(out)
    })
}

/// The newest `limit` stored entries of agent `cid`, oldest first.
pub fn page(db: &Db, cid: u64, limit: usize) -> Result<Vec<StoredEntry>, StoreError> {
    db.read(|conn| {
        let def = def();
        let width = def.key.len() + def.columns.len();
        let mut stmt = conn.prepare_cached(TRANSCRIPT_PAGE)?;
        let mut rows = stmt.query([to_i64(cid), to_i64(limit)])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let mut values = Vec::with_capacity(width);
            for i in 0..width {
                values.push(r.get::<_, SqlValue>(i)?);
            }
            let columns = values.split_off(def.key.len());
            let row = Row {
                key: values,
                columns,
            };
            out.push(StoredEntry::take(def, &row, &Loaded::default())?);
        }
        out.reverse();
        Ok(out)
    })
}

/// Loads every agent's newest [`RESTORE_PER_AGENT`] stored entries into `store`, and attaches `db` so what is recorded
/// from now on is written to it.
pub fn restore(store: &Store, db: &Arc<Db>) -> Result<usize, StoreError> {
    let cids = cids(db)?;
    let mut loaded = 0;
    for cid in &cids {
        let entries = page(db, *cid, RESTORE_PER_AGENT)?;
        loaded += entries.len();
        store.preload(*cid, entries);
    }
    store.attach(Arc::clone(db));
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base(event: &str) -> SandboxBase {
        SandboxBase {
            event: event.to_string(),
            sandbox: "sb-1".to_string(),
            cid: 7,
            template: "base".to_string(),
            by: "owner".to_string(),
        }
    }

    /// One entry of every kind, with the fields filled the way the call sites fill them.
    fn every_kind() -> Vec<Entry> {
        vec![
            Entry::WorkStart {
                kind: WorkKind::Chat,
                thread: "conv-1".into(),
                prompt: "Write the README".into(),
                origin: json!({"owner": true, "mcp": false, "acp": false}),
            },
            Entry::WorkEnd {
                status: Outcome::Done,
                summary: "wrote it".into(),
            },
            Entry::WorkInterrupted {
                reason: "its machine is stopping".into(),
            },
            Entry::ModelRequest {
                model: "m".into(),
                messages: 3,
                tools: json!(["read", "bash"]),
                last_user: "hi".into(),
            },
            Entry::ModelResponse {
                via: None,
                model: "p/m".into(),
                content: Some(json!([{"type": "text", "text": "hello"}])),
                stop: Some(json!("end_turn")),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cache_read_tokens: 2,
                    cache_write_tokens: 1,
                },
                cost_usd: None,
            },
            Entry::ModelChargeShortfall {
                model: "m".into(),
                task: None,
                wanted: 5,
                charged: 2,
                available: 2,
                error: Some("short".into()),
            },
            Entry::ToolRequest {
                call: "c1".into(),
                name: "read".into(),
                input: json!({"path": "a.md"}),
            },
            Entry::ToolDecision {
                call: "c1".into(),
                decision: Verdict::Allow,
                output: String::new(),
            },
            Entry::ToolResult {
                call: "c1".into(),
                ok: true,
                output: "42".into(),
            },
            Entry::EngineUpdate(
                json!({"call": "e1", "item": "message", "message_id": null, "text": "hi"}),
            ),
            Entry::Error {
                message: "boom".into(),
            },
            Entry::Compressed {
                before: 100,
                after: 40,
                folded: 3,
                reason: "auto".into(),
            },
            Entry::Egress {
                host: "example.com".into(),
                port: 443,
                allowed: true,
                bytes_up: 10,
                bytes_down: 20,
                ms: 5,
            },
            Entry::EgressInject {
                host: "api.example.com".into(),
                method: "POST".into(),
                path: "/v1".into(),
                rule: "r1".into(),
            },
            Entry::McpCall {
                server: "docs".into(),
                tool: "search".into(),
                ms: 12,
                ok: true,
                output_bytes: 900,
            },
            Entry::Artifact {
                id: "a1".into(),
                version: 2,
                title: "Plan".into(),
                kind: "markdown".into(),
            },
            Entry::Decision {
                provider: "anthropic".into(),
                model: "claude".into(),
                questions: 2,
                input_tokens: 77,
            },
            Entry::Control {
                control: json!({"action": "steer", "from": "owner", "text": "focus"}),
            },
            Entry::VmCheckpoint {
                checkpoint: Some(json!({"name": "cp1"})),
                auto: Some(true),
                deleted: None,
            },
            Entry::VmRestore {
                name: "cp1".into(),
                safety: Value::Null,
            },
            Entry::ForkedFrom {
                cid: 3,
                checkpoint: "cp1".into(),
            },
            Entry::Fork {
                forks: json!([{"cid": 9, "run": "r9"}]),
                checkpoint: "cp1".into(),
            },
            Entry::MoveStarted {
                from: "node-a".into(),
                to: "node-b".into(),
            },
            Entry::Moved {
                from: "node-a".into(),
                to: "node-b".into(),
                bytes: 4096,
                ms: 33,
            },
            Entry::Payment {
                task: "t1".into(),
                amount: 500,
                spent: 1000,
            },
            Entry::Wake {},
            Entry::Sleep {
                snapshot: json!({"id": "s1"}),
            },
            Entry::ShareRequest {
                artifact: "route-1".into(),
                title: "Deck".into(),
                approval: "ap-1".into(),
            },
            Entry::ShareDecision {
                text: "approved".into(),
                approved: true,
                title: "Deck".into(),
            },
            Entry::SandboxCreate {
                base: base("create"),
                timeout: 300,
                allow_internet_access: false,
            },
            Entry::SandboxKill {
                base: base("kill"),
                reason: "done".into(),
                kept_for_snapshots: Some(2),
            },
            Entry::SandboxPause {
                base: base("pause"),
                reason: "idle".into(),
            },
            Entry::SandboxResume {
                base: base("resume"),
                timeout: 60,
            },
            Entry::SandboxSnapshot {
                base: base("snapshot_delete"),
                snapshot: "snap-1".into(),
            },
            Entry::SandboxNetwork {
                base: base("network_pending"),
                allow_out: json!(["a.example"]),
                deny_out: json!([]),
                allow_internet_access: true,
                why: Some("needs approval".into()),
            },
        ]
    }

    fn json_of(stored: &StoredEntry) -> Value {
        serde_json::to_value(stored).unwrap()
    }

    /// The hub's store in a fresh folder (every file migrated, the way the hub opens it). The folder is returned with
    /// the store: dropping either one stops the writers.
    fn hub_store(label: &str) -> (tempfile::TempDir, apz_store::Store) {
        let dir = tempfile::Builder::new().prefix(label).tempdir().unwrap();
        let store = crate::store::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn every_kind_round_trips_through_the_store_page() {
        let (_dir, files) = hub_store("transcript-kinds");
        let db = files.db(crate::store::TRANSCRIPTS);
        let store = Store::default();
        let bus = Bus::default();
        store.attach(Arc::clone(db));
        let kinds = every_kind();
        assert_eq!(kinds.len(), 35, "one sample for each of the 35 kinds");
        for (i, entry) in kinds.iter().enumerate() {
            let work = 1 + (i as u64 % 3);
            store.record(&bus, 7, work, entry.clone());
        }
        db.flush().unwrap();
        let back = page(db, 7, 1000).unwrap();
        let want: Vec<StoredEntry> = store.last(7, usize::MAX);
        assert_eq!(back.len(), kinds.len());
        for (got, want) in back.iter().zip(&want) {
            assert_eq!(json_of(got), json_of(want), "seq {}", want.seq);
        }
        let kinds_back: Vec<String> = back
            .iter()
            .map(|e| json_of(e)["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds_back.iter().collect::<HashSet<_>>().len(), 35);
    }

    #[test]
    fn the_api_json_of_a_few_kinds_is_what_it_was() {
        let stored = |seq: u64, entry: Entry| StoredEntry {
            cid: 7,
            seq,
            session: None,
            work: 2,
            at_ms: 100,
            entry,
        };
        assert_eq!(
            json_of(&stored(
                1,
                Entry::WorkStart {
                    kind: WorkKind::Task,
                    thread: "conv-1".into(),
                    prompt: "p".into(),
                    origin: json!({"task": "t1", "from_cid": null, "owner": false, "mcp": false, "acp": false}),
                }
            )),
            json!({
                "cid": 7, "seq": 1, "work": 2, "at_ms": 100, "kind": "work_start",
                "data": {"kind": "task", "thread": "conv-1", "prompt": "p",
                         "origin": {"task": "t1", "from_cid": null, "owner": false, "mcp": false, "acp": false}}
            })
        );
        assert_eq!(
            json_of(&stored(
                2,
                Entry::ModelResponse {
                    via: None,
                    model: "p/m".into(),
                    content: Some(json!([{"type": "text", "text": "hi"}])),
                    stop: Some(json!("end_turn")),
                    usage: Usage {
                        input_tokens: 3,
                        output_tokens: 1,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                    cost_usd: None,
                }
            )),
            json!({
                "cid": 7, "seq": 2, "work": 2, "at_ms": 100, "kind": "model_response",
                "data": {"model": "p/m", "content": [{"type": "text", "text": "hi"}], "stop": "end_turn",
                         "usage": {"input_tokens": 3, "output_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0}}
            })
        );
        assert_eq!(
            json_of(&stored(
                3,
                Entry::ModelResponse {
                    via: Some("gateway".into()),
                    model: "m".into(),
                    content: None,
                    stop: None,
                    usage: Usage {
                        input_tokens: 5,
                        output_tokens: 2,
                        cache_read_tokens: 1,
                        cache_write_tokens: 0,
                    },
                    cost_usd: Some(0.25),
                }
            )),
            json!({
                "cid": 7, "seq": 3, "work": 2, "at_ms": 100, "kind": "model_response",
                "data": {"via": "gateway", "model": "m",
                         "usage": {"input_tokens": 5, "output_tokens": 2, "cache_read_tokens": 1, "cache_write_tokens": 0},
                         "cost_usd": 0.25}
            })
        );
        assert_eq!(
            json_of(&stored(
                4,
                Entry::VmCheckpoint {
                    checkpoint: None,
                    auto: None,
                    deleted: Some("cp1".into()),
                }
            )),
            json!({"cid": 7, "seq": 4, "work": 2, "at_ms": 100, "kind": "vm_checkpoint", "data": {"deleted": "cp1"}})
        );
        assert_eq!(
            json_of(&stored(5, Entry::Wake {})),
            json!({"cid": 7, "seq": 5, "work": 2, "at_ms": 100, "kind": "wake", "data": {}})
        );
        assert_eq!(
            json_of(&stored(
                6,
                Entry::SandboxNetwork {
                    base: base("network"),
                    allow_out: json!(["a"]),
                    deny_out: json!([]),
                    allow_internet_access: false,
                    why: None,
                }
            )),
            json!({
                "cid": 7, "seq": 6, "work": 2, "at_ms": 100, "kind": "sandbox_network",
                "data": {"event": "network", "sandbox": "sb-1", "cid": 7, "template": "base", "by": "owner",
                         "allowOut": ["a"], "denyOut": [], "allow_internet_access": false}
            })
        );
    }

    #[test]
    fn a_page_of_a_thousand_entries_reads_in_order() {
        let (_dir, files) = hub_store("transcript-page");
        let db = files.db(crate::store::TRANSCRIPTS);
        let store = Store::default();
        let bus = Bus::default();
        store.attach(Arc::clone(db));
        for i in 0..1000u64 {
            store.record(
                &bus,
                7,
                1,
                Entry::Error {
                    message: format!("entry {i}"),
                },
            );
        }
        db.flush().unwrap();
        let all = page(db, 7, 1000).unwrap();
        assert_eq!(all.len(), 1000);
        assert!(all.windows(2).all(|w| w[0].seq < w[1].seq), "oldest first");
        assert_eq!(all[999].seq, 1000);
        let newest = page(db, 7, 3).unwrap();
        assert_eq!(
            newest.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![998, 999, 1000]
        );
        assert_eq!(cids(db).unwrap(), vec![7]);
        store.wipe(7);
        db.flush().unwrap();
        assert!(page(db, 7, 10).unwrap().is_empty());
        assert!(cids(db).unwrap().is_empty());
    }

    #[test]
    fn restore_loads_the_newest_entries_and_continues_the_sequence() {
        let (_dir, files) = hub_store("transcript-restore");
        let db = files.db(crate::store::TRANSCRIPTS);
        let writer = Store::default();
        let bus = Bus::default();
        writer.attach(Arc::clone(db));
        for _ in 0..3 {
            writer.record(&bus, 4, 1, Entry::Wake {});
        }
        db.flush().unwrap();
        let fresh = Store::default();
        assert_eq!(restore(&fresh, db).unwrap(), 3);
        let next = fresh.record(&bus, 4, 1, Entry::Wake {});
        assert_eq!(next, 4, "the sequence goes on after the stored entries");
        assert_eq!(fresh.last(4, 10).len(), 4);
    }

    #[test]
    fn a_thread_keeps_its_own_work_items_turns() {
        let (store, bus) = (Store::default(), Bus::default());
        let start = |thread: &str| Entry::WorkStart {
            kind: WorkKind::Chat,
            thread: thread.into(),
            prompt: String::new(),
            origin: json!({}),
        };
        store.record(&bus, 9, 1, start("conv-1"));
        store.record(&bus, 9, 2, start("conv-2"));
        store.record(&bus, 9, 1, Entry::Wake {});
        store.record(&bus, 9, 2, Entry::Wake {});
        let last = store.record(
            &bus,
            9,
            1,
            Entry::WorkEnd {
                status: Outcome::Done,
                summary: String::new(),
            },
        );
        let first: Vec<(u64, String)> = store
            .on_thread(9, "conv-1", 0, 10)
            .into_iter()
            .map(|e| (e.work, json_of(&e)["kind"].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(
            first,
            [
                (1, "work_start".to_string()),
                (1, "wake".to_string()),
                (1, "work_end".to_string())
            ]
        );
        assert_eq!(store.on_thread(9, "conv-2", 0, 10).len(), 2);
        assert!(store.on_thread(9, "conv-1", last, 10).is_empty());
        assert!(store.on_thread(9, "conv-3", 0, 10).is_empty());
        assert!(store.on_thread(4, "conv-1", 0, 10).is_empty());
    }

    #[test]
    fn entries_are_ordered_capped_and_searchable() {
        let (store, bus) = (Store::default(), Bus::default());
        let first = store.record(
            &bus,
            7,
            1,
            Entry::WorkStart {
                kind: WorkKind::Task,
                thread: "t".into(),
                prompt: "Write the README".into(),
                origin: json!({}),
            },
        );
        store.record(
            &bus,
            7,
            1,
            Entry::WorkEnd {
                status: Outcome::Done,
                summary: String::new(),
            },
        );
        store.record(&bus, 8, 2, Entry::Wake {});
        assert_eq!(store.after(7, 0, 10).len(), 2);
        assert_eq!(store.after(7, first, 10).len(), 1);
        assert_eq!(store.search(7, "readme", 10).len(), 1);
        assert!(store.after(9, 0, 10).is_empty());
    }

    #[test]
    fn last_is_the_newest_entries_in_order() {
        let (store, bus) = (Store::default(), Bus::default());
        for i in 0..5 {
            store.record(
                &bus,
                7,
                1,
                Entry::Error {
                    message: i.to_string(),
                },
            );
        }
        store.record(&bus, 8, 2, Entry::Wake {});
        let newest = store.last(7, 2);
        assert_eq!(
            newest
                .iter()
                .map(|e| json_of(e)["data"]["message"].as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            vec!["3".to_string(), "4".to_string()]
        );
        assert!(newest[0].seq < newest[1].seq);
        assert_eq!(
            store.last(7, 99).len(),
            5,
            "a window longer than the log returns all of it"
        );
        assert_eq!(store.last(7, 0).len(), 0);
        assert!(store.last(9, 3).is_empty());
    }

    #[test]
    fn the_entry_cap_still_holds() {
        let (store, bus) = (Store::default(), Bus::default());
        let mut newest = 0;
        for _ in 0..KEEP + 10 {
            newest = store.record(&bus, 7, 1, Entry::Wake {});
        }
        assert_eq!(store.last(7, usize::MAX).len(), KEEP);
        assert_eq!(store.last(7, 1)[0].seq, newest);
    }

    #[test]
    fn a_window_stays_under_its_byte_budget() {
        let (store, bus) = (Store::default(), Bus::default());
        let big = "x".repeat(64 << 10);
        for _ in 0..200 {
            store.record(
                &bus,
                7,
                1,
                Entry::ToolResult {
                    call: "c".into(),
                    ok: true,
                    output: big.clone(),
                },
            );
        }
        let rows: Vec<StoredEntry> = (1..=200u64)
            .map(|seq| StoredEntry {
                cid: 8,
                seq,
                session: None,
                work: 1,
                at_ms: 0,
                entry: Entry::ToolResult {
                    call: "c".into(),
                    ok: true,
                    output: big.clone(),
                },
            })
            .collect();
        store.preload(8, rows);
        for cid in [7u64, 8] {
            let (len, bytes) = {
                let windows = store.windows();
                (windows[&cid].len(), windows[&cid].bytes)
            };
            assert!(
                bytes <= MAX_BYTES_PER_AGENT,
                "cid {cid} holds {bytes} bytes"
            );
            assert!(
                (15..200).contains(&len),
                "cid {cid} keeps what fits: {len} entries"
            );
            let summed: usize = store
                .last(cid, usize::MAX)
                .iter()
                .map(|e| size_of(&e.entry))
                .sum();
            assert_eq!(summed, bytes, "the tracked size matches the entries kept");
        }
    }

    #[test]
    fn the_newest_entry_is_kept_even_over_the_budget() {
        let (store, bus) = (Store::default(), Bus::default());
        for _ in 0..10 {
            store.record(&bus, 7, 1, Entry::Wake {});
        }
        let huge = "y".repeat(2 * MAX_BYTES_PER_AGENT);
        let seq = store.record(
            &bus,
            7,
            1,
            Entry::ToolResult {
                call: "c".into(),
                ok: true,
                output: huge,
            },
        );
        let kept = store.last(7, usize::MAX);
        assert_eq!(
            kept.len(),
            1,
            "older entries go once the newest is over budget"
        );
        assert_eq!(kept[0].seq, seq);

        let next = store.record(
            &bus,
            7,
            1,
            Entry::WorkEnd {
                status: Outcome::Done,
                summary: String::new(),
            },
        );
        let kept = store.last(7, usize::MAX);
        assert_eq!(
            kept.len(),
            1,
            "the oversized entry goes once a newer one arrives"
        );
        assert_eq!(kept[0].seq, next);
    }

    #[test]
    fn the_migration_creates_the_table_as_defined() {
        let migration = include_str!("../sql/transcripts/0001_init.sql");
        assert!(
            migration.contains(&def().ddl()),
            "0001_init.sql must hold the DDL of transcript.rs's table:\n{}",
            def().ddl()
        );
    }

    #[test]
    fn after_pages_from_the_cursor_in_seq_order() {
        let mut window = Window::default();
        // 4 is pushed before 3, as a record racing another on the same agent can be.
        for seq in [1u64, 2, 4, 3, 5] {
            window.push(
                StoredEntry {
                    cid: 7,
                    seq,
                    session: None,
                    work: 1,
                    at_ms: 0,
                    entry: Entry::Wake {},
                },
                1,
            );
        }
        let seqs = |after: u64| -> Vec<u64> { window.after_seq(after).map(|e| e.seq).collect() };
        for after in 0..=6 {
            let filtered: Vec<u64> = window
                .iter()
                .filter(|e| e.seq > after)
                .map(|e| e.seq)
                .collect();
            assert_eq!(seqs(after), filtered, "after {after}");
        }
        assert_eq!(seqs(2), vec![3, 4, 5]);
        assert!(seqs(5).is_empty());
    }
}
