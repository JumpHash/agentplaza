//! The node daemon's JSON API, spoken over a tiny blocking HTTP/1.1 client.
//!
//! The dashboard reaches `noded` on loopback only, so this is deliberately not
//! a general HTTP client: one request per connection, no redirects, no
//! chunked bodies, no TLS, no async runtime, no client dependency — just a
//! `TcpStream` per call, the same shape `crates/noded/tests/common/mod.rs`
//! uses to test the daemon.
//!
//! The types here mirror `noded`'s views (`registry::RunView`, `VmView`,
//! `RunsView`), its ledger (`tasks::TaskView`) and its treasury
//! (`treasury::TreasuryView`, `treasury::JournalView`), and the requests it
//! accepts; the states and kinds are enums so an unknown one from a newer
//! daemon cannot fail the parse, it renders as `Unknown`.

use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// The port `noded` listens on unless `--url` says otherwise.
pub const DEFAULT_PORT: u16 = 8899;

/// How long one request may take, end to end.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a status line and its headers may occupy, together.
pub(crate) const MAX_HEAD: usize = 64 * 1024;

/// The most a response body may occupy. A run's serial transcript is the
/// largest thing the daemon serves, and it is bounded in memory there, so this
/// is a ceiling, not a budget.
const MAX_BODY: usize = 16 * 1024 * 1024;

/// A run's state, as `core.runs.state` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    /// The run exists and its VMs are being booted.
    Starting,
    /// Every VM of the run is up.
    Running,
    /// The run ended because its VMs stopped by themselves.
    Done,
    /// The run ended because its VMs could not be booted.
    Failed,
    /// The run ended because the host killed its VMs.
    Killed,
    /// A state this dashboard does not know; rendered as-is, never a parse error.
    #[serde(other)]
    Unknown,
}

impl RunState {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            RunState::Starting => "starting",
            RunState::Running => "running",
            RunState::Done => "done",
            RunState::Failed => "failed",
            RunState::Killed => "killed",
            RunState::Unknown => "unknown",
        }
    }

    /// Whether the run has not ended, so killing it is still an action.
    pub fn is_live(self) -> bool {
        matches!(self, RunState::Starting | RunState::Running)
    }

    /// The word a stamp presses into the register: the name, set in caps.
    pub fn stamp_label(self) -> &'static str {
        match self {
            RunState::Starting => "STARTING",
            RunState::Running => "RUNNING",
            RunState::Done => "DONE",
            RunState::Failed => "FAILED",
            RunState::Killed => "KILLED",
            RunState::Unknown => "UNKNOWN",
        }
    }
}

/// A VM's state, as the API names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    /// The VM is up.
    Running,
    /// The VM stopped by itself.
    Exited,
    /// The VM was stopped by the host.
    Killed,
    /// A state this dashboard does not know.
    #[serde(other)]
    Unknown,
}

impl VmState {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            VmState::Running => "running",
            VmState::Exited => "exited",
            VmState::Killed => "killed",
            VmState::Unknown => "unknown",
        }
    }

    /// The word a stamp presses into the register.
    pub fn stamp_label(self) -> &'static str {
        match self {
            VmState::Running => "UP",
            VmState::Exited => "EXITED",
            VmState::Killed => "KILLED",
            VmState::Unknown => "UNKNOWN",
        }
    }
}

/// One VM of a run, as `GET /api/v0/runs/{id}` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Vm {
    /// The VM's index among the run's VMs.
    pub id: u32,
    /// The VM's vsock guest CID; absent when the run has no vsock devices.
    #[serde(default)]
    pub guest_cid: Option<u64>,
    /// The role this VM's guest runs as, when it runs as one (the daemon's
    /// `noded --agents` fleet boots one long-running VM per role and stamps
    /// each of its VM rows): absent for every VM that was not started as a
    /// role, and absent from an older daemon that does not report it yet.
    #[serde(default)]
    pub role: Option<String>,
    /// The VM's state.
    pub state: VmState,
    /// The VM's serial capture; present only in a single-run GET.
    #[serde(default)]
    pub serial: Option<String>,
}

/// A run, as the API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Run {
    /// The run's id (`r<seq>`).
    pub id: String,
    /// The run's state.
    pub state: RunState,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// Heartbeats before the guest reboots itself, or `null`.
    #[serde(default)]
    pub beats: Option<u32>,
    /// When the run was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When the run ended, in milliseconds since the Unix epoch, or `null`.
    #[serde(default)]
    pub ended_at: Option<u64>,
    /// The run's VMs.
    pub vms: Vec<Vm>,
}

impl Run {
    /// How many of the run's VMs are still up.
    pub fn live_vms(&self) -> usize {
        self.vms
            .iter()
            .filter(|vm| vm.state == VmState::Running)
            .count()
    }

    /// The base guest CID the run's vsock devices were given, when it has any.
    pub fn base_cid(&self) -> Option<u64> {
        self.vms.first().and_then(|vm| vm.guest_cid)
    }

    /// The distinct roles the run's VMs carry, in the order their VMs appear:
    /// one entry for a run booted as a single role, every role for the
    /// daemon's role fleet, and none for a run whose VMs carry no role.
    pub fn roles(&self) -> Vec<&str> {
        let mut roles: Vec<&str> = Vec::new();
        for vm in &self.vms {
            if let Some(role) = vm.role.as_deref() {
                if !roles.contains(&role) {
                    roles.push(role);
                }
            }
        }
        roles
    }

    /// The run's single role, when exactly one distinct role is carried: what
    /// a register row stamps. `None` when no VM carries a role, and also when
    /// several roles are carried (the register prints the count instead).
    pub fn role(&self) -> Option<&str> {
        let mut found: Option<&str> = None;
        for vm in &self.vms {
            match vm.role.as_deref() {
                None => {}
                Some(role) => match found {
                    None => found = Some(role),
                    Some(known) if known == role => {}
                    Some(_) => return None,
                },
            }
        }
        found
    }

    /// The run's serial transcript: the first VM's capture, or empty when the
    /// view carries none (the list endpoint omits captures).
    pub fn serial(&self) -> &str {
        self.vms
            .first()
            .and_then(|vm| vm.serial.as_deref())
            .unwrap_or("")
    }
}

/// `GET /api/v0/runs`: the list endpoint's body.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunsPage {
    /// Every run, oldest first.
    pub runs: Vec<Run>,
}

/// `GET /api/v0/machine`: the node's machine, as `noded` reports it
/// (`noded::MachineView`; `core.machines`'s fields plus the vsock transport).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Machine {
    /// The host's hostname.
    pub name: String,
    /// The host architecture (`x86_64`/`aarch64`).
    pub arch: String,
    /// The VMM backend (`kvm`/`hvf`).
    pub backend: String,
    /// The interrupt controller in use (`ioapic`, `vgicv2`, …).
    pub irqchip: String,
    /// The machine class per the schema (`server`/`apple`/…).
    pub class: String,
    /// The host's CPU count.
    pub cpus: u32,
    /// The host's total memory, in MiB.
    pub memory_mib: u64,
    /// The vsock transport (`uds-muxer`).
    pub vsock_mode: String,
    /// When the daemon started, in milliseconds since the Unix epoch.
    pub started_at_ms: u64,
}

impl Machine {
    /// The machine model as one line: what the substrate is made of.
    pub fn model_label(&self) -> String {
        format!(
            "{} · {}/{} · {} cpus · {} MiB",
            self.name, self.arch, self.backend, self.cpus, self.memory_mib
        )
    }
}

/// `GET /status` on a `vsockd --status-port` surface: the host facts the hub
/// serves, one row per connected keepalive agent, and the live session count.
///
/// The shape is frozen by the fleet contract (`crates/vsockd/src/status.rs`),
/// and this parses it field for field: a field the body does not carry is an
/// error that names it, and a field this client does not know is ignored
/// rather than a parse failure — a newer hub may add to its body without
/// breaking the console.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HubStatus {
    /// The hub's own facts.
    pub node: HubNode,
    /// One row per connected agent, ordered by CID.
    pub agents: Vec<HubAgent>,
    /// How many sessions are live.
    pub sessions_active: usize,
}

/// The hub's host facts, as `/status` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HubNode {
    /// The guest CIDs the hub serves (one Unix socket per CID).
    pub cids: Vec<u32>,
    /// The vsock port every CID is served on.
    pub port: u32,
    /// How long the daemon has been up, in seconds.
    pub uptime_s: u64,
    /// The most sessions the hub will accept at once.
    pub max_sessions: usize,
    /// How long a keepalive session may go unheard before it is dropped.
    pub keepalive_idle_s: u64,
}

/// One connected agent, as `/status` reports it: a keepalive session the hub
/// accepted at a handshake and holds until its CID goes quiet.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HubAgent {
    /// The guest CID the session was accepted for: the key the register's own
    /// VM rows join it on.
    pub cid: u32,
    /// The role the hub accepted at the handshake.
    pub role: String,
    /// When the session connected, in milliseconds since the Unix epoch.
    pub connected_at_ms: u64,
    /// When the most recent heartbeat arrived, in milliseconds since the Unix
    /// epoch.
    pub last_heartbeat_ms: u64,
    /// How many tool calls the session has served.
    pub calls: u64,
}

impl HubAgent {
    /// How long ago this agent last beat, at `now_ms`.
    pub fn heartbeat_age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_heartbeat_ms)
    }

    /// How long this agent has been connected, at `now_ms`.
    pub fn connected_for_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.connected_at_ms)
    }
}

impl HubStatus {
    /// The hub's row for one guest CID, when it reports one connected.
    pub fn agent(&self, cid: u64) -> Option<&HubAgent> {
        let cid = u32::try_from(cid).ok()?;
        self.agents.iter().find(|agent| agent.cid == cid)
    }

    /// The hub's uptime as a duration label, from its whole seconds.
    pub fn uptime_ms(&self) -> u64 {
        self.node.uptime_s.saturating_mul(1_000)
    }
}

/// One `POST /tell` answer from the hub's admin surface: the agent's own `ok`
/// and the tool's `output`.
///
/// The hub writes `{"ok": <bool>, "output": "<text>"}`. `ok: true` is the tool
/// succeeding and `ok: false` the tool declining — both are the agent
/// *answering*, and its answer is data. A *refusal of the call itself* (a role
/// that does not grant the tool, a CID that is not connected, a timeout) never
/// reaches this shape: it is an HTTP status and an `ApiError::Status` instead,
/// carrying the hub's own words.
///
/// The parse is permissive by construction: a field this console does not know
/// is ignored, while a missing `ok` or `output` is an error that names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TellAnswer {
    /// Whether the tool succeeded (`false` is the tool declining, its reason
    /// in `output`).
    pub ok: bool,
    /// The tool's output — the agent's own words, displayed and never
    /// interpreted.
    pub output: String,
}

/// What one `POST /stop` on the hub's admin surface produced.
///
/// The surface answers every stop it accepts with `200` and one of two shapes,
/// so this is the whole of an accepted stop: the session ended, or the wait
/// lapsed with the session still up. A CID the hub does not serve (404) or has
/// no live session for (409) is an `ApiError::Status` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// The session ended cleanly. `calls` is what the guest's `bye` counted,
    /// or the session's own served-call count when the end did not come by a
    /// `bye`.
    Stopped {
        /// The calls the ended session had served.
        calls: u64,
    },
    /// The wait lapsed and the session is still up: **nothing was torn down**,
    /// and the hub's own reason says so.
    StillUp {
        /// The hub's own words for the lapse.
        reason: String,
    },
}

/// The `POST /stop` body the surface writes: `stopped` is the tag, and the
/// branch's own field sits beside it. Read defensively — `stopped` is required
/// (a body without it is not a stop answer), while `calls` and `reason` are
/// read by the branch they belong to.
#[derive(Debug, Deserialize)]
struct StopAnswer {
    stopped: bool,
    #[serde(default)]
    calls: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
}

impl StopAnswer {
    /// The outcome this body describes, or an error naming what the branch it
    /// chose is missing.
    fn into_outcome(self) -> Result<StopOutcome, ApiError> {
        if self.stopped {
            let calls = self.calls.ok_or_else(|| {
                ApiError::Malformed(
                    "the stop answer says stopped but names no calls count".to_string(),
                )
            })?;
            Ok(StopOutcome::Stopped { calls })
        } else {
            let reason = self.reason.ok_or_else(|| {
                ApiError::Malformed(
                    "the stop answer says not stopped but gives no reason".to_string(),
                )
            })?;
            Ok(StopOutcome::StillUp { reason })
        }
    }
}

/// A column the daemon always writes, whose value may be `null`.
///
/// `noded`'s task view writes every column a task has, so an **absent** key is
/// not the same fact as a `null` one: absent means the body is not the shape
/// this client reads, and it is named rather than quietly read as "none".
///
/// serde reads a plain `Option<T>` field's absence as `None` — its
/// `missing_field` hands the field to `deserialize_option`, which answers
/// `visit_none` — so the distinction would be erased by the obvious type. This
/// wraps the value and reads it through `serde_json::Value`, whose
/// `deserialize_any` an absent key refuses (a missing field is an error that
/// names the column) while a `null` becomes `None`. It dereferences to
/// `Option<T>`, so `as_deref`, `copied`, `cloned` and `is_some` read as they
/// would on the plain type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column<T>(Option<T>);

impl<T> Column<T> {
    /// The column holding `value`.
    pub fn filled(value: Option<T>) -> Self {
        Self(value)
    }

    /// The column's value, if the daemon sent one.
    pub fn get(&self) -> Option<&T> {
        self.0.as_ref()
    }

    /// The column's value, owned.
    pub fn into_option(self) -> Option<T> {
        self.0
    }
}

impl<T: Copy> Column<T> {
    /// The column's value, copied out of it.
    pub fn copied(&self) -> Option<T> {
        self.0
    }
}

impl<T: Clone> Column<T> {
    /// The column's value, cloned out of it.
    pub fn cloned(&self) -> Option<T> {
        self.0.clone()
    }
}

impl<T> std::ops::Deref for Column<T> {
    type Target = Option<T>;

    fn deref(&self) -> &Option<T> {
        &self.0
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Column<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // `Value`'s own `Deserialize` asks for `deserialize_any`, which the
        // pseudo-value an absent key is never answers — so this errors naming
        // the column, where an `Option<T>` field would have read `None`.
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Null => Ok(Column(None)),
            value => T::deserialize(value)
                .map(|value| Column(Some(value)))
                .map_err(serde::de::Error::custom),
        }
    }
}

/// A task's state, as `noded` and the schema's `core.tasks` name it.
///
/// The daemon's v0 ledger moves a task only along the pairs it accepts
/// (`open` → `claimed`, then `done`, `failed` or `withdrawn`); a state this
/// console does not know parses as [`TaskState::Unknown`] rather than failing
/// the row, exactly as an unknown run or VM state does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// Created and claimable.
    Open,
    /// Bound to a live agent.
    Claimed,
    /// Ended: the agent reports the work done.
    Done,
    /// Ended: the agent reports the work failed.
    Failed,
    /// Ended: given up before it finished.
    Withdrawn,
    /// A state this dashboard does not know; rendered as-is, never a parse
    /// error.
    #[serde(other)]
    Unknown,
}

impl TaskState {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            TaskState::Open => "open",
            TaskState::Claimed => "claimed",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Withdrawn => "withdrawn",
            TaskState::Unknown => "unknown",
        }
    }

    /// The word a stamp presses into the ledger, in caps.
    pub fn stamp_label(self) -> &'static str {
        match self {
            TaskState::Open => "OPEN",
            TaskState::Claimed => "CLAIMED",
            TaskState::Done => "DONE",
            TaskState::Failed => "FAILED",
            TaskState::Withdrawn => "WITHDRAWN",
            TaskState::Unknown => "UNKNOWN",
        }
    }

    /// Whether the task has ended: these are the states the schema pairs with
    /// a non-null `outcome`, and a state this console does not know is a newer
    /// daemon's word rather than a settled one.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            TaskState::Done | TaskState::Failed | TaskState::Withdrawn
        )
    }

    /// Whether a claim would be accepted: only an open task can be claimed.
    pub fn is_claimable(self) -> bool {
        self == TaskState::Open
    }

    /// Whether a finish would be accepted: only a claimed task can be
    /// finished.
    pub fn is_finishable(self) -> bool {
        self == TaskState::Claimed
    }

    /// Whether a task in this state can still be withdrawn: either state
    /// before it ends.
    pub fn is_withdrawable(self) -> bool {
        matches!(self, TaskState::Open | TaskState::Claimed)
    }
}

/// How a task ended: the schema's outcome enum, set when a task closes and
/// `null` while it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskOutcome {
    /// The work is done (the task ended `done`).
    Accepted,
    /// The work failed (the task ended `failed`).
    Failed,
    /// The task was given up (the task ended `withdrawn`).
    Cancelled,
    /// An outcome this dashboard does not know.
    #[serde(other)]
    Unknown,
}

impl TaskOutcome {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            TaskOutcome::Accepted => "accepted",
            TaskOutcome::Failed => "failed",
            TaskOutcome::Cancelled => "cancelled",
            TaskOutcome::Unknown => "unknown",
        }
    }
}

/// One task, as `GET /api/v0/tasks` and `GET /api/v0/tasks/{id}` report it
/// (`noded::tasks::TaskView`).
///
/// Every column the daemon writes is required — a body missing one is an error
/// that names it, at every level — while a column this client does not know is
/// ignored rather than a parse failure, so a newer daemon may add to its view
/// without breaking the console. The columns that may be `null` are
/// [`Column`]s: `null` reads as "none", an absent key does not.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Task {
    /// The task's id (`t<seq>`).
    pub id: String,
    /// What the task is for (the schema's `title`).
    pub goal: String,
    /// How the work will be judged complete (the schema's `spec_ref`), or
    /// `null`.
    pub acceptance: Column<String>,
    /// The task's budget, in credits (the schema's `budget_credits`).
    pub budget: u64,
    /// The role the task was created for, or `null`.
    pub role: Column<String>,
    /// The task's state.
    pub state: TaskState,
    /// How the task ended, or `null` while it lives.
    pub outcome: Column<TaskOutcome>,
    /// The schema's `assignee_id`: the claiming agent's role, or `cid:<n>` for
    /// an agent without one.
    pub assignee_id: Column<String>,
    /// The schema's `verifier_id`; `null` at v0, which records no verifier.
    pub verifier_id: Column<String>,
    /// The claiming agent's guest CID, or `null` while nobody claimed it.
    pub agent_cid: Column<u64>,
    /// What the agent said when it finished, or `null`.
    pub note: Column<String>,
    /// When the task was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When the task last changed, in milliseconds since the Unix epoch.
    pub updated_at: u64,
}

/// `GET /api/v0/tasks`: the list endpoint's body.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TasksPage {
    /// Every task, oldest first.
    pub tasks: Vec<Task>,
}

/// A task's artifact, as `POST /api/v0/tasks/{id}/artifact` reports it
/// (`noded`'s `ArtifactView`): the file the task's agent read back out of its
/// own workspace, and the terms its `read_file` returned it in.
///
/// Every column the daemon writes is required — a body missing one is an
/// error that names it — while a column this client does not know is ignored,
/// so a newer daemon may add to its view without breaking the console. None of
/// these columns is nullable: the CID is the one the daemon resolved the
/// task's assignee to, and a read is always reported with its own size and
/// truncation. So this needs no [`Column`] — the nullable columns are
/// [`Task`]'s, and this body has none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskArtifact {
    /// The task whose agent read the file.
    pub task: String,
    /// The guest CID of the agent that read it (the task's assignee).
    pub agent_cid: u64,
    /// The path the caller asked for, as it sent it.
    pub path: String,
    /// The text the agent read.
    pub content: String,
    /// The file's size on disk, in bytes, as the agent reported it.
    pub size: u64,
    /// Whether the agent's read cap, rather than the end of the file, ended
    /// the content.
    pub truncated: bool,
}

/// `POST /api/v0/tasks/{id}/artifact`: the body names the file to read
/// (`noded`'s `ArtifactRequest`).
///
/// The daemon drives the task's agent's `read_file` with this path, so where a
/// path may point and what it may reach is the agent's harness's rule, not
/// this client's: this sends the path as given and the refusal, when there is
/// one, is the agent's own words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ArtifactRequest {
    /// The path of the file to read, as the task's agent wrote it.
    path: String,
}

/// An account's kind, the schema's `treasury.account_kind`
/// (`noded::treasury::AccountKind`).
///
/// The type is complete — every kind the schema lists — but a daemon may open
/// only some of them: v0's lifecycle posts `mint` and `task_escrow` and leaves
/// the rest for the entries that need them. A kind this console does not know
/// parses as [`AccountKind::Unknown`] rather than failing the row, exactly as
/// an unknown run or task state does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// The source of all credits; every mint and refund names it.
    Mint,
    /// The plaza's own balance.
    Treasury,
    /// A town's budget.
    TownBudget,
    /// A task's escrow: a budget minted into it, refunded when the task ends.
    TaskEscrow,
    /// An agent's earnings.
    AgentEarnings,
    /// Metered real cost.
    ExternalSpend,
    /// A kind this dashboard does not know; rendered as-is, never a parse
    /// error.
    #[serde(other)]
    Unknown,
}

impl AccountKind {
    /// The name the API uses, the schema's own spelling.
    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Mint => "mint",
            AccountKind::Treasury => "treasury",
            AccountKind::TownBudget => "town_budget",
            AccountKind::TaskEscrow => "task_escrow",
            AccountKind::AgentEarnings => "agent_earnings",
            AccountKind::ExternalSpend => "external_spend",
            AccountKind::Unknown => "unknown",
        }
    }
}

/// An entry's kind, the schema's `treasury.entry_kind`
/// (`noded::treasury::EntryKind`).
///
/// The schema lists nine; v0's task lifecycle posts only `mint` and `refund`,
/// and the rules for the rest are enforced all the same. A kind this console
/// does not know parses as [`EntryKind::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Owner or council funds a task budget.
    Mint,
    /// Metered real cost: escrow → external_spend.
    Spend,
    /// A parent escrow locks a price in a child task's escrow.
    Subcontract,
    /// Verified work: child escrow → contractor earnings.
    Payment,
    /// Unused escrow back to the parent escrow or the mint.
    Refund,
    /// Warranty claim: contractor earnings → escrow or treasury.
    Clawback,
    /// Sanction: earnings → treasury.
    Fine,
    /// Appeal filing fee.
    Fee,
    /// Owner or council moves budget between towns.
    BudgetMove,
    /// A kind this dashboard does not know; rendered as-is, never a parse
    /// error.
    #[serde(other)]
    Unknown,
}

impl EntryKind {
    /// The name the API uses, the schema's own spelling.
    pub fn label(self) -> &'static str {
        match self {
            EntryKind::Mint => "mint",
            EntryKind::Spend => "spend",
            EntryKind::Subcontract => "subcontract",
            EntryKind::Payment => "payment",
            EntryKind::Refund => "refund",
            EntryKind::Clawback => "clawback",
            EntryKind::Fine => "fine",
            EntryKind::Fee => "fee",
            EntryKind::BudgetMove => "budget_move",
            EntryKind::Unknown => "unknown",
        }
    }
}

/// One account, as `GET /api/v0/treasury` reports it
/// (`noded::treasury::AccountView`).
///
/// Every column the daemon writes is required — none of these is nullable — so
/// a body that omits one is an error that names it, while a column this client
/// does not know is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Account {
    /// The account's id.
    pub id: u64,
    /// The account's kind.
    pub kind: AccountKind,
    /// The account's owner (`owner_ref`): a task id, an agent id, or a fixed
    /// name.
    pub owner_ref: String,
    /// The account's credit limit (0 at v0: no reputation slice sets one).
    pub credit_limit: i64,
    /// The balance: the sum of the account's postings, derived by the daemon
    /// and never stored.
    pub balance: i128,
    /// When the account was opened, in milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// The treasury view's totals line (`noded::treasury::TotalsView`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Totals {
    /// How many accounts the treasury holds.
    pub accounts: usize,
    /// How many entries the journal holds.
    pub entries: usize,
    /// The total credited across the journal.
    pub credits: i128,
    /// The total debited across the journal.
    pub debits: i128,
    /// The sum of every account's balance: zero while the journal balances.
    pub balance: i128,
}

/// One posting of an entry (`noded::treasury::PostingView`).
///
/// The amount's sign *is* the side: negative is a debit, positive a credit,
/// exactly as the schema reads it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Posting {
    /// The account's id.
    pub account: u64,
    /// The account's kind.
    pub kind: AccountKind,
    /// The account's owner.
    pub owner_ref: String,
    /// The amount: negative for a debit, positive for a credit.
    pub amount: i64,
}

impl Posting {
    /// Whether this posting is on the debit side (`amount < 0`).
    pub fn is_debit(&self) -> bool {
        self.amount < 0
    }

    /// The posting's size, whichever side it is on.
    pub fn magnitude(&self) -> i64 {
        self.amount.unsigned_abs() as i64
    }
}

/// One journal entry (`noded::treasury::EntryView`): the schema's
/// `treasury.entries` row together with its postings.
///
/// `task_id` and `root_task` are the two nullable columns — `null` for an
/// entry that belongs to no task (a `fine` or a `fee`) — so they are
/// [`Column`]s, while every other column is required.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Entry {
    /// The entry's id, assigned once in order.
    pub id: u64,
    /// The entry's kind.
    pub kind: EntryKind,
    /// The task the entry belongs to, or `null`.
    pub task_id: Column<String>,
    /// The entry's chain root, or `null` when it belongs to no chain.
    pub root_task: Column<String>,
    /// The entry's memo.
    pub memo: String,
    /// Who posted it (always `treasury-service`).
    pub posted_by: String,
    /// When it was posted, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// The entry's size: the total of its credit-side postings, which equals
    /// the total of its debits, since every entry balances to zero.
    pub amount: i128,
    /// The entry's postings, which sum to zero.
    pub postings: Vec<Posting>,
}

/// `GET /api/v0/treasury`: the accounts and the totals
/// (`noded::treasury::TreasuryView`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Treasury {
    /// The accounts, in creation order, each with its derived balance.
    pub accounts: Vec<Account>,
    /// The totals line.
    pub totals: Totals,
    /// Real money spent (0 at v0: nothing meters spend yet).
    pub spent: i128,
    /// The daemon's own note on why `spent` is what it is.
    pub note: String,
}

/// `GET /api/v0/treasury/journal`: the journal
/// (`noded::treasury::JournalView`).
///
/// The endpoint returns the newest `limit` entries (oldest of those first), so
/// `truncated` is how a caller tells a journal read whole from one that was
/// cut to its bound.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Journal {
    /// The entries the response carries, in chronological order (newest last).
    pub entries: Vec<Entry>,
    /// How many entries the journal holds.
    pub total: usize,
    /// The limit the response was cut to.
    pub limit: usize,
    /// Whether the journal holds more entries than the response carries.
    pub truncated: bool,
}

/// The whole treasury as the console reads it: the accounts and their totals,
/// and the journal beside them.
///
/// Both are served by the same daemon and are two halves of one fact, so the
/// console reads them together: a read is the treasury view and the journal
/// view, and a failure of either is a read that did not happen. That is what
/// lets the pane have one schedule and one failure count of its own, exactly
/// as the hub's status surface does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreasuryRead {
    /// The accounts and their totals.
    pub treasury: Treasury,
    /// The journal, newest last and bounded by the daemon's own limit.
    pub journal: Journal,
}

/// `POST /api/v0/tasks`: the body sends exactly the fields `noded` accepts
/// (`noded::tasks::CreateTask`); a field it does not name is absent rather
/// than null, and the daemon fills its own default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTask {
    /// What the task is for.
    pub goal: String,
    /// How the work will be judged complete, or `null`.
    pub acceptance: Option<String>,
    /// The task's budget, in credits, or `null` for the schema's zero.
    pub budget: Option<u64>,
    /// The role the task is for, or `null`.
    pub role: Option<String>,
}

/// `POST /api/v0/tasks/{id}/claim`: the body names the guest CID the claiming
/// agent is connected as (`noded::tasks::ClaimRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimTask {
    /// The guest CID of the live agent making the claim.
    pub agent_cid: u64,
}

/// How a finishing agent reports the task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishOutcome {
    /// The work is done.
    Done,
    /// The work failed.
    Failed,
}

impl FinishOutcome {
    /// The name the API uses.
    pub fn label(self) -> &'static str {
        match self {
            FinishOutcome::Done => "done",
            FinishOutcome::Failed => "failed",
        }
    }
}

/// `POST /api/v0/tasks/{id}/finish`: the body sends the outcome and, when the
/// agent says anything, its note (`noded::tasks::FinishRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishTask {
    /// How the task ended.
    pub outcome: FinishOutcome,
    /// What the agent has to say about it, or `null`.
    pub note: Option<String>,
}

/// `POST /api/v0/runs/{id}/input`: the body writes one line to one VM's
/// console, in exactly the fields `noded` accepts (`noded::InputRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRequest {
    /// The line (or bytes) to write to the VM's console.
    pub data: String,
    /// The index of the VM to write to.
    pub vm: u32,
}

/// `POST /api/v0/runs`: the body sends exactly the fields `noded` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRun {
    /// How many VMs to boot.
    pub n: u32,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// Heartbeats before the guest reboots itself; `null` runs the guest
    /// long-lived.
    pub beats: Option<u32>,
    /// Base guest CID for the run's vsock devices; `null` boots without vsock.
    pub vsock_cid: Option<u64>,
}

/// Where the daemon is. Loopback only, by construction.
///
/// `Hash` is what identifies the event stream's subscription: the same
/// endpoint keeps the same stream running, and a different one starts a new
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    /// The literal loopback host: always `127.0.0.1`.
    host: String,
    /// The daemon's port.
    port: u16,
}

impl Endpoint {
    /// The default endpoint: the daemon on `127.0.0.1:8899`.
    pub fn loopback() -> Self {
        Self {
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: DEFAULT_PORT,
        }
    }

    /// An endpoint on a loopback host, rejecting anything else.
    ///
    /// Exact matching only: `127.0.0.1` (or `localhost`, which is normalized
    /// to it). No other name, address, or lookalike is accepted, and the host
    /// is never resolved through DNS.
    pub fn loopback_at(host: &str, port: u16) -> Result<Self, ApiError> {
        let host = match host {
            "127.0.0.1" => Ipv4Addr::LOCALHOST.to_string(),
            "localhost" => Ipv4Addr::LOCALHOST.to_string(),
            other => {
                return Err(ApiError::Config(format!(
                    "{other:?} is not a loopback host: the dashboard reaches the node \
                     daemon on 127.0.0.1 only"
                )));
            }
        };
        if port == 0 {
            return Err(ApiError::Config(
                "port 0 is not a port to reach the node daemon on".to_string(),
            ));
        }
        Ok(Self { host, port })
    }

    /// Parses `http://127.0.0.1[:port]` (a bare `host[:port]` is accepted
    /// too): the daemon serves its API at the root of a known default port.
    pub fn from_url(url: &str) -> Result<Self, ApiError> {
        Self::parse_url(url, Surface::Daemon)
    }

    /// Parses the hub's status url: the same parsing and the same
    /// loopback-only policy, with the port required.
    ///
    /// `vsockd`'s status surface is off unless it was started with
    /// `--status-port`, so there is no default to fall back to — a bare host
    /// would silently poll whatever happens to hold the daemon's port.
    pub fn hub_url(url: &str) -> Result<Self, ApiError> {
        Self::parse_url(url, Surface::HubStatus)
    }

    /// Parses the hub's admin url: the same parsing and the same
    /// loopback-only policy as the status surface, with the port required.
    ///
    /// `vsockd`'s admin surface is off unless it was started with
    /// `--admin-port`, so there is no default to fall back to and a bare host
    /// is refused by name — the operator's own second process, reached over
    /// the loopback exactly as the status surface is.
    pub fn hub_admin_url(url: &str) -> Result<Self, ApiError> {
        Self::parse_url(url, Surface::HubAdmin)
    }

    /// Parses `host[:port]` (an `http://` prefix is accepted too), filling a
    /// missing port from the surface's default — or, when it has none,
    /// refusing the url because its caller needs the port named.
    fn parse_url(url: &str, surface: Surface) -> Result<Self, ApiError> {
        let url = url.trim();
        let rest = match url.strip_prefix("http://") {
            Some(rest) => rest,
            None if url.contains("://") => {
                return Err(ApiError::Config(format!(
                    "{url:?} is not an http:// URL"
                )));
            }
            None => url,
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(ApiError::Config(format!(
                "{url:?} carries a path, query or fragment; the daemon serves its API at the root"
            )));
        }
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port)) => {
                let port = port.parse::<u16>().map_err(|_| {
                    ApiError::Config(format!("{port:?} is not a port"))
                })?;
                (host, port)
            }
            None => match surface.default_port() {
                Some(port) => (rest, port),
                None => {
                    return Err(ApiError::Config(format!(
                        "{url:?} names no port: {}",
                        surface.portless_reason()
                    )));
                }
            },
        };
        Self::loopback_at(host, port)
    }

    /// The endpoint's host, always `127.0.0.1`.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The endpoint's port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The endpoint as a URL, for the header and error messages.
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Self::loopback()
    }
}

/// Which loopback surface a url names. It decides the two things the three
/// endpoints differ on: whether a missing port may be filled from a default,
/// and — when it may not — why the url must name its port.
///
/// The daemon holds a well-known port, so `--url 127.0.0.1` means something.
/// The hub's status and admin surfaces are each off unless their own flag
/// asked for them, so a bare host there would silently poll whatever happens
/// to hold the daemon's port; naming the surface makes the refusal say which
/// flag turns it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The node daemon: its API is served at the root of a well-known port.
    Daemon,
    /// The hub's read-only status surface (`vsockd --status-port`).
    HubStatus,
    /// The hub's admin surface (`vsockd --admin-port`): `/tell` and `/stop`.
    HubAdmin,
}

impl Surface {
    /// The port a url for this surface may fall back to, if any.
    fn default_port(self) -> Option<u16> {
        match self {
            Surface::Daemon => Some(DEFAULT_PORT),
            Surface::HubStatus | Surface::HubAdmin => None,
        }
    }

    /// Why a url naming this surface must carry its port.
    fn portless_reason(self) -> &'static str {
        match self {
            Surface::Daemon => "the daemon serves its API on a default port",
            Surface::HubStatus => {
                "the hub's status surface is off unless it was started with --status-port, \
                 so its url must carry the port it serves on"
            }
            Surface::HubAdmin => {
                "the hub's admin surface is off unless it was started with --admin-port, \
                 so its url must carry the port it serves on"
            }
        }
    }
}

/// Why a call did not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The endpoint or a request parameter cannot be used as given.
    Config(String),
    /// The daemon could not be reached.
    Connect(String),
    /// The daemon answered, but not with something this client can read.
    Malformed(String),
    /// The daemon answered with a non-2xx status and `{"error": ...}`.
    Status {
        /// The HTTP status.
        status: u16,
        /// The daemon's message.
        message: String,
    },
}

impl ApiError {
    /// The HTTP status of a refusal, when there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Whether the daemon says the thing asked about does not exist.
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }

    /// Whether the daemon could not be reached at all.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, ApiError::Connect(_))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Config(message) => write!(formatter, "{message}"),
            ApiError::Connect(message) => write!(formatter, "cannot reach {message}"),
            ApiError::Malformed(message) => {
                write!(formatter, "the daemon's answer is malformed: {message}")
            }
            ApiError::Status { status, message } => write!(formatter, "{status}: {message}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// A client for one daemon endpoint.
#[derive(Debug, Clone)]
pub struct Client {
    endpoint: Endpoint,
    timeout: Duration,
}

impl Client {
    /// A client for `endpoint`, with the default timeout.
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The same client with a different request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The endpoint this client talks to.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// `GET /api/v0/runs`.
    pub fn runs(&self) -> Result<RunsPage, ApiError> {
        self.json("GET", "/api/v0/runs", None)
    }

    /// `GET /api/v0/machine`: the node as a machine.
    pub fn machine(&self) -> Result<Machine, ApiError> {
        self.json("GET", "/api/v0/machine", None)
    }

    /// `GET /status` on the hub's status surface: who is connected right now,
    /// and the host facts the hub serves them with.
    ///
    /// The same client, a different endpoint: the hub is a second read-only
    /// process on a second loopback port, and its body is not the daemon's.
    pub fn hub_status(&self) -> Result<HubStatus, ApiError> {
        self.json("GET", "/status", None)
    }

    /// `POST /tell` on the hub's admin surface: drive one tool on the agent
    /// connected as `cid`.
    ///
    /// The hub writes the agent's own answer as `{"ok": <bool>, "output":
    /// "<text>"}` — `ok: false` is the tool declining, which is still an
    /// answer. The call *refusals* are the hub's own statuses, carried through
    /// unchanged in its own words: `403` is the session's role not granting the
    /// tool (decided before anything reaches the agent), `400` a request
    /// outside the body's contract, `404` a CID the hub does not serve, `409`
    /// a served CID with no live session, and `504` an agent that did not
    /// answer within `timeout_ms`.
    ///
    /// It is a POST, not a GET, and deliberately so: it runs a real tool in the
    /// agent's session — a real cost, a real refusal mode and a real effect on
    /// the agent's turn — so a request a prefetch or a crawler could replay
    /// must not be advertised as repeat-safe.
    pub fn hub_tell(
        &self,
        cid: u64,
        name: &str,
        args: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<TellAnswer, ApiError> {
        let body = serde_json::json!({
            "cid": cid,
            "name": name,
            "args": args,
            "timeout_ms": timeout_ms,
        })
        .to_string();
        let reply = self.send("POST", "/tell", Some(&body))?;
        if !(200..300).contains(&reply.status) {
            return Err(reply.into_error());
        }
        serde_json::from_str(&reply.body)
            .map_err(|err| ApiError::Malformed(format!("POST /tell: {err}")))
    }

    /// `POST /stop` on the hub's admin surface: ask the agent connected as
    /// `cid` to end its session.
    ///
    /// Every stop the hub accepts is a `200`, carrying either `stopped: true`
    /// with the calls the guest's `bye` counted, or `stopped: false` with the
    /// hub's own reason — the wait lapsed and **nothing was torn down**. A CID
    /// the hub does not serve is a `404` and a served CID with no live session
    /// a `409`, each in the hub's own words.
    ///
    /// No reason is sent: the hub's own default (`the operator stopped this
    /// session`) is the honest one, and the operator supplies none.
    pub fn hub_stop(&self, cid: u64) -> Result<StopOutcome, ApiError> {
        let body = serde_json::json!({ "cid": cid }).to_string();
        let reply = self.send("POST", "/stop", Some(&body))?;
        if !(200..300).contains(&reply.status) {
            return Err(reply.into_error());
        }
        serde_json::from_str::<StopAnswer>(&reply.body)
            .map_err(|err| ApiError::Malformed(format!("POST /stop: {err}")))?
            .into_outcome()
    }

    /// `GET /api/v0/tasks`: the plaza's ledger, oldest task first.
    pub fn tasks(&self) -> Result<TasksPage, ApiError> {
        self.json("GET", "/api/v0/tasks", None)
    }

    /// `GET /api/v0/tasks/{id}`: one task.
    pub fn task(&self, id: &str) -> Result<Task, ApiError> {
        self.json("GET", &format!("/api/v0/tasks/{}", checked_id(id)?), None)
    }

    /// `POST /api/v0/tasks`: files a task, which the daemon answers `201` with
    /// in its first state, `open`.
    pub fn create_task(&self, create: &CreateTask) -> Result<Task, ApiError> {
        let body = serde_json::to_string(create).map_err(|err| {
            ApiError::Malformed(format!("cannot serialize the create request: {err}"))
        })?;
        self.json("POST", "/api/v0/tasks", Some(&body))
    }

    /// `POST /api/v0/tasks/{id}/claim`: binds an open task to the live agent
    /// connected as `agent_cid`.
    ///
    /// The daemon checks that CID against the register the moment the request
    /// arrives, so a CID no live agent holds is refused `409` — in its own
    /// words, which this client carries through unchanged.
    pub fn claim_task(&self, id: &str, agent_cid: u64) -> Result<Task, ApiError> {
        let body = serde_json::to_string(&ClaimTask { agent_cid }).map_err(|err| {
            ApiError::Malformed(format!("cannot serialize the claim request: {err}"))
        })?;
        self.json(
            "POST",
            &format!("/api/v0/tasks/{}/claim", checked_id(id)?),
            Some(&body),
        )
    }

    /// `POST /api/v0/tasks/{id}/finish`: ends a claimed task as `done` or
    /// `failed`, with the agent's note when it has one.
    pub fn finish_task(
        &self,
        id: &str,
        outcome: FinishOutcome,
        note: Option<&str>,
    ) -> Result<Task, ApiError> {
        let body = serde_json::to_string(&FinishTask {
            outcome,
            note: note.map(str::to_string),
        })
        .map_err(|err| ApiError::Malformed(format!("cannot serialize the finish request: {err}")))?;
        self.json(
            "POST",
            &format!("/api/v0/tasks/{}/finish", checked_id(id)?),
            Some(&body),
        )
    }

    /// `DELETE /api/v0/tasks/{id}`: withdraws a task before it ends.
    pub fn withdraw_task(&self, id: &str) -> Result<Task, ApiError> {
        self.json("DELETE", &format!("/api/v0/tasks/{}", checked_id(id)?), None)
    }

    /// `POST /api/v0/tasks/{id}/artifact`: reads one file the task's agent
    /// wrote into its own workspace, by driving that agent's `read_file`
    /// through the daemon's hub admin surface.
    ///
    /// It is a POST, not a GET, and deliberately so: it drives the agent's
    /// session — a tool call with a real cost, a real refusal mode and a real
    /// effect on the agent's turn — so it must not be replayed by a prefetch,
    /// a retry or a crawler.
    ///
    /// The refusal statuses are the daemon's own and are carried through
    /// unchanged, in its own words: `403` is the hub's role policy, `422` the
    /// agent's own refusal (a directory, a path outside the workspace, a file
    /// that is not UTF-8 text), `409` a task with no assignee or an agent no
    /// longer connected, `400` a path this endpoint cannot carry, and `503` a
    /// daemon never told where the hub's admin surface is.
    pub fn task_artifact(&self, id: &str, path: &str) -> Result<TaskArtifact, ApiError> {
        let body = serde_json::to_string(&ArtifactRequest {
            path: path.to_string(),
        })
        .map_err(|err| {
            ApiError::Malformed(format!("cannot serialize the artifact request: {err}"))
        })?;
        self.json(
            "POST",
            &format!("/api/v0/tasks/{}/artifact", checked_id(id)?),
            Some(&body),
        )
    }

    /// The treasury: `GET /api/v0/treasury` and `GET /api/v0/treasury/journal`,
    /// read together as one read.
    ///
    /// The two endpoints are two halves of one fact — the accounts with their
    /// derived balances, and the append-only entries that moved them — so a
    /// read is both or neither: a failure of either half is a read that did
    /// not happen, which is what gives the pane one schedule and one failure
    /// count. Nothing here writes: every entry is posted by the daemon's own
    /// treasury service, and this client has no path that would post one.
    pub fn treasury(&self) -> Result<TreasuryRead, ApiError> {
        let treasury: Treasury = self.json("GET", "/api/v0/treasury", None)?;
        let journal: Journal = self.json("GET", "/api/v0/treasury/journal", None)?;
        Ok(TreasuryRead { treasury, journal })
    }

    /// `GET /api/v0/runs/{id}`, with every VM's serial capture.
    pub fn run(&self, id: &str) -> Result<Run, ApiError> {
        self.json("GET", &format!("/api/v0/runs/{}", checked_id(id)?), None)
    }

    /// `POST /api/v0/runs/{id}/input`: writes `data` to VM `vm`'s serial
    /// console. The daemon terminates the line when the caller did not.
    pub fn input(&self, id: &str, vm: u32, data: &str) -> Result<Run, ApiError> {
        let body = serde_json::to_string(&InputRequest {
            data: data.to_string(),
            vm,
        })
        .map_err(|err| ApiError::Malformed(format!("cannot serialize the input request: {err}")))?;
        self.json(
            "POST",
            &format!("/api/v0/runs/{}/input", checked_id(id)?),
            Some(&body),
        )
    }

    /// `DELETE /api/v0/runs/{id}`: kills the run's VMs.
    pub fn kill(&self, id: &str) -> Result<Run, ApiError> {
        self.json("DELETE", &format!("/api/v0/runs/{}", checked_id(id)?), None)
    }

    /// `POST /api/v0/runs`: boots a run's VMs.
    pub fn spawn(&self, create: &CreateRun) -> Result<Run, ApiError> {
        let body = serde_json::to_string(create)
            .map_err(|err| ApiError::Malformed(format!("cannot serialize the create request: {err}")))?;
        self.json("POST", "/api/v0/runs", Some(&body))
    }

    /// One request, parsed as JSON on success and as `{"error": ...}` otherwise.
    fn json<T: DeserializeOwned>(
        &self,
        method: &str,
        target: &str,
        body: Option<&str>,
    ) -> Result<T, ApiError> {
        let reply = self.send(method, target, body)?;
        if !(200..300).contains(&reply.status) {
            return Err(reply.into_error());
        }
        serde_json::from_str(&reply.body)
            .map_err(|err| ApiError::Malformed(format!("{method} {target}: {err}")))
    }

    /// Sends one request and reads the whole response.
    fn send(&self, method: &str, target: &str, body: Option<&str>) -> Result<Reply, ApiError> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.endpoint.port));
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)
            .map_err(|err| ApiError::Connect(format!("{}: {err}", self.endpoint.url())))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|err| ApiError::Connect(format!("{}: {err}", self.endpoint.url())))?;
        let _ = stream.set_nodelay(true);

        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: {}:{}\r\n",
            self.endpoint.host, self.endpoint.port
        );
        if let Some(body) = body {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("Connection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .and_then(|()| match body {
                Some(body) => stream.write_all(body.as_bytes()),
                None => Ok(()),
            })
            .and_then(|()| stream.flush())
            .map_err(|err| ApiError::Connect(format!("{}: {err}", self.endpoint.url())))?;
        // The daemon reads exactly `Content-Length` bytes, so this only tells
        // it the request is over; it never waits for it either way.
        let _ = stream.shutdown(Shutdown::Write);

        read_reply(&mut stream)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new(Endpoint::default())
    }
}

/// A run or task id as it may appear in a path: plain, so it cannot change the
/// request.
fn checked_id(id: &str) -> Result<&str, ApiError> {
    if !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Ok(id)
    } else {
        Err(ApiError::Config(format!(
            "{id:?} is not a daemon id: ids are plain alphanumeric words"
        )))
    }
}

/// One response, read off the socket.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The refusal this response carries, in the answering surface's own
    /// words.
    ///
    /// The daemon writes `{"error": "..."}` for every refusal. The hub's admin
    /// surface writes the same for most, but answers a role-policy refusal
    /// (403) with its tell body `{"ok":false,"output":"role <r> does not grant
    /// <tool>"}` — so the message is the string `error` when present, else the
    /// string `output`, else the whole body. Nothing is invented: a body that
    /// is not one of these shapes arrives as it was written.
    fn into_error(self) -> ApiError {
        #[derive(Deserialize)]
        struct Refusal {
            #[serde(default)]
            error: Option<String>,
            #[serde(default)]
            output: Option<String>,
        }
        let message = serde_json::from_str::<Refusal>(&self.body)
            .ok()
            .and_then(|body| body.error.or(body.output))
            .unwrap_or_else(|| self.body.trim().to_string());
        ApiError::Status {
            status: self.status,
            message,
        }
    }
}

/// Reads one HTTP/1.1 response: the head, then exactly `Content-Length` bytes.
fn read_reply(stream: &mut TcpStream) -> Result<Reply, ApiError> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(position) = find(&buffer, b"\r\n\r\n") {
            break position + 4;
        }
        if buffer.len() > MAX_HEAD {
            return Err(ApiError::Malformed(
                "the response head is too large".to_string(),
            ));
        }
        let read = stream
            .read(&mut chunk)
            .map_err(|err| ApiError::Malformed(format!("reading the response: {err}")))?;
        if read == 0 {
            return Err(ApiError::Malformed(
                "the daemon closed before answering".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buffer[..head_end - 4]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            ApiError::Malformed(format!("no status code in {status_line:?}"))
        })?;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ApiError::Malformed(
                "chunked responses are not supported".to_string(),
            ));
        }
    }
    let content_length = content_length.ok_or_else(|| {
        ApiError::Malformed("the response has no Content-Length".to_string())
    })?;
    if content_length > MAX_BODY {
        return Err(ApiError::Malformed(format!(
            "the response body is larger than {MAX_BODY} bytes"
        )));
    }

    let mut body = buffer[head_end..].to_vec();
    while body.len() < content_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|err| ApiError::Malformed(format!("reading the response: {err}")))?;
        if read == 0 {
            return Err(ApiError::Malformed(
                "the response body is shorter than Content-Length".to_string(),
            ));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Reply {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// The first position of `needle` in `haystack`.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parses a fixture body, so the tests exercise exactly the type the client
/// deserializes into.
#[cfg(test)]
fn from_body<T: DeserializeOwned>(body: &str) -> T {
    serde_json::from_str(body).expect("the fixture should parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list body as `GET /api/v0/runs` sends it: no serial captures, and a
    /// run with vsock devices next to one without.
    const LIST: &str = r#"{"runs":[
        {"id":"r1","state":"running","vcpus":2,"memory_mib":512,"beats":null,
         "created_at":1760000000000,"ended_at":null,
         "vms":[{"id":0,"guest_cid":50,"state":"running"},
                {"id":1,"guest_cid":51,"state":"running"}]},
        {"id":"r2","state":"killed","vcpus":1,"memory_mib":128,"beats":3,
         "created_at":1760000001000,"ended_at":1760000009000,
         "vms":[{"id":0,"state":"killed"}]}
    ]}"#;

    /// A single-run body as `GET /api/v0/runs/r1` sends it: every VM's serial.
    const DETAIL: &str = "{\"id\":\"r1\",\"state\":\"running\",\"vcpus\":2,\"memory_mib\":512,\
         \"beats\":null,\"created_at\":1760000000000,\"ended_at\":null,\
         \"vms\":[{\"id\":0,\"guest_cid\":50,\"state\":\"running\",\
         \"serial\":\"login: plaza\\r\\n$ uname\\r\\nLinux\\r\\n\"}]}";

    #[test]
    fn a_list_body_parses_into_runs() {
        let page: RunsPage = from_body(LIST);
        assert_eq!(page.runs.len(), 2);

        let first = &page.runs[0];
        assert_eq!(first.id, "r1");
        assert_eq!(first.state, RunState::Running);
        assert_eq!(first.vcpus, 2);
        assert_eq!(first.memory_mib, 512);
        assert_eq!(first.beats, None);
        assert_eq!(first.created_at, 1_760_000_000_000);
        assert_eq!(first.ended_at, None);
        assert_eq!(first.live_vms(), 2);
        assert_eq!(first.base_cid(), Some(50));
        assert_eq!(first.vms[1].guest_cid, Some(51));
        assert_eq!(first.serial(), "", "a list view carries no serial");

        let second = &page.runs[1];
        assert_eq!(second.state, RunState::Killed);
        assert_eq!(second.beats, Some(3));
        assert_eq!(second.ended_at, Some(1_760_000_009_000));
        assert_eq!(second.live_vms(), 0);
        assert_eq!(second.base_cid(), None, "no vsock device, no CID");
    }

    #[test]
    fn a_detail_body_parses_into_a_run_with_its_serial() {
        let run: Run = from_body(DETAIL);
        assert_eq!(run.id, "r1");
        assert_eq!(run.vms[0].guest_cid, Some(50));
        assert_eq!(run.vms[0].serial.as_deref(), Some("login: plaza\r\n$ uname\r\nLinux\r\n"));
        assert_eq!(run.serial().lines().count(), 3);
    }

    /// A role run's body as the daemon reports it once `noded --agents` has
    /// booted the initial setup: VM rows that each carry their role (the
    /// daemon's fleet is one run of them).
    const ROLES: &str = r#"{"id":"r1","state":"running","vcpus":1,"memory_mib":128,
        "beats":null,"created_at":1760000000000,"ended_at":null,
        "vms":[{"id":0,"guest_cid":100,"role":"builder","state":"running"},
               {"id":1,"guest_cid":101,"role":"reviewer","state":"running"},
               {"id":2,"guest_cid":102,"state":"exited"}]}"#;

    #[test]
    fn a_role_on_a_vm_row_parses_and_a_missing_one_is_none() {
        let run: Run = from_body(ROLES);
        assert_eq!(run.vms[0].role.as_deref(), Some("builder"));
        assert_eq!(run.vms[1].role.as_deref(), Some("reviewer"));
        assert_eq!(
            run.vms[2].role, None,
            "a VM row with no role is a VM that is not a role agent"
        );
        assert_eq!(
            from_body::<RunsPage>(LIST).runs[0].vms[0].role,
            None,
            "an older daemon's rows carry no role field at all, and that is not a parse error"
        );
    }

    #[test]
    fn a_runs_roles_are_distinct_and_in_vm_order() {
        let run: Run = from_body(ROLES);
        assert_eq!(run.roles(), vec!["builder", "reviewer"]);
        assert_eq!(
            run.role(),
            None,
            "two distinct roles are what the register prints as a count"
        );

        // A run whose VMs all share one role is the run that gets its name
        // stamped; a repeated role is still one role.
        let single: Run = from_body(
            r#"{"id":"r2","state":"running","vcpus":1,"memory_mib":128,"created_at":1,
                "ended_at":null,
                "vms":[{"id":0,"role":"builder","state":"running"},
                       {"id":1,"role":"builder","state":"running"}]}"#,
        );
        assert_eq!(single.roles(), vec!["builder"]);
        assert_eq!(single.role(), Some("builder"));
        assert_eq!(
            from_body::<RunsPage>(LIST).runs[0].role(),
            None,
            "a run with no roles is not a role run"
        );
    }

    #[test]
    fn an_unknown_state_parses_instead_of_failing() {
        let body = r#"{"runs":[{"id":"r9","state":"hibernating","vcpus":1,"memory_mib":128,
            "created_at":1,"ended_at":null,"vms":[{"id":0,"state":"paused"}]}]}"#;
        let page: RunsPage = from_body(body);
        assert_eq!(page.runs[0].state, RunState::Unknown);
        assert_eq!(page.runs[0].vms[0].state, VmState::Unknown);
        assert!(!page.runs[0].state.is_live());
    }

    #[test]
    fn a_create_body_carries_exactly_the_daemons_fields() {
        let create = CreateRun {
            n: 2,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            vsock_cid: Some(50),
        };
        let body = serde_json::to_value(create).expect("serializable");
        assert_eq!(
            body,
            serde_json::json!({
                "n": 2, "vcpus": 2, "memory_mib": 512, "beats": null, "vsock_cid": 50
            })
        );
        let parsed: CreateRun = from_body(&body.to_string());
        assert_eq!(parsed, create);
    }

    #[test]
    fn the_default_endpoint_is_the_loopbacks_daemon() {
        let endpoint = Endpoint::default();
        assert_eq!(endpoint.host(), "127.0.0.1");
        assert_eq!(endpoint.port(), DEFAULT_PORT);
        assert_eq!(endpoint.url(), "http://127.0.0.1:8899");
    }

    #[test]
    fn a_url_selects_a_loopback_port() {
        let endpoint = Endpoint::from_url("http://127.0.0.1:9000").expect("loopback");
        assert_eq!(endpoint.host(), "127.0.0.1");
        assert_eq!(endpoint.port(), 9000);

        assert_eq!(Endpoint::from_url("127.0.0.1:9000").expect("bare host").port(), 9000);
        assert_eq!(Endpoint::from_url("http://127.0.0.1/").expect("trailing slash").port(), 8899);
        assert_eq!(Endpoint::from_url(" http://localhost ").expect("localhost").host(), "127.0.0.1");
    }

    #[test]
    fn a_non_loopback_url_is_refused() {
        for url in [
            "http://0.0.0.0:8899",
            "http://127.0.0.1.evil.tld:8899",
            "https://127.0.0.1:8899",
            "http://[::1]:8899",
            "http://192.168.1.10:8899",
        ] {
            let error = Endpoint::from_url(url).expect_err("must be refused");
            assert!(
                matches!(error, ApiError::Config(_)),
                "{url} should be a configuration error, got {error:?}"
            );
        }
    }

    #[test]
    fn a_url_with_a_path_query_or_portless_garbage_is_refused() {
        assert!(Endpoint::from_url("http://127.0.0.1:8899/api/v0/runs").is_err());
        assert!(Endpoint::from_url("http://127.0.0.1:8899?a=b").is_err());
        assert!(Endpoint::from_url("http://127.0.0.1:notaport").is_err());
        assert!(Endpoint::from_url("http://127.0.0.1:0").is_err());
    }

    #[test]
    fn a_run_id_that_is_not_plain_is_refused_before_it_reaches_a_path() {
        assert_eq!(checked_id("r12").expect("a plain id"), "r12");
        for id in ["", "r1/../r2", "r1?x=1", "r1 r2", "r\u{00e9}"] {
            assert!(matches!(checked_id(id), Err(ApiError::Config(_))), "{id:?}");
        }
        let client = Client::default();
        assert!(client.run("r1/../r2").is_err());
    }

    #[test]
    fn an_error_body_becomes_the_daemons_message() {
        let reply = Reply {
            status: 400,
            body: r#"{"error":"n must be between 1 and 64"}"#.to_string(),
        };
        let error = reply.into_error();
        assert_eq!(error.status(), Some(400));
        assert_eq!(error.to_string(), "400: n must be between 1 and 64");
        assert!(!error.is_not_found());
    }

    #[test]
    fn a_404_is_recognizable() {
        let reply = Reply {
            status: 404,
            body: r#"{"error":"no run with id r9"}"#.to_string(),
        };
        let error = reply.into_error();
        assert!(error.is_not_found());
        assert_eq!(error.to_string(), "404: no run with id r9");
    }

    #[test]
    fn a_non_json_error_body_still_reports_the_status() {
        let reply = Reply {
            status: 500,
            body: "boom\n".to_string(),
        };
        let error = reply.into_error();
        assert_eq!(error.to_string(), "500: boom");
    }

    #[test]
    fn find_locates_the_head_terminator() {
        assert_eq!(find(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
        assert_eq!(find(b"abc", b"\r\n\r\n"), None);
    }

    #[test]
    fn a_connect_error_says_so() {
        let error = ApiError::Connect("http://127.0.0.1:8899: refused".to_string());
        assert!(error.is_unreachable());
        assert_eq!(error.to_string(), "cannot reach http://127.0.0.1:8899: refused");
    }

    #[test]
    fn a_machine_body_parses_into_the_nodes_machine() {
        let machine: Machine = from_body(
            r#"{"name":"zarathos-pc","arch":"x86_64","backend":"kvm","irqchip":"ioapic",
                "class":"server","cpus":24,"memory_mib":128320,"vsock_mode":"uds-muxer",
                "started_at_ms":1791650569674}"#,
        );
        assert_eq!(machine.name, "zarathos-pc");
        assert_eq!(machine.arch, "x86_64");
        assert_eq!(machine.backend, "kvm");
        assert_eq!(machine.irqchip, "ioapic");
        assert_eq!(machine.class, "server");
        assert_eq!(machine.cpus, 24);
        assert_eq!(machine.memory_mib, 128_320);
        assert_eq!(machine.vsock_mode, "uds-muxer");
        assert_eq!(machine.started_at_ms, 1_791_650_569_674);
        assert_eq!(
            machine.model_label(),
            "zarathos-pc · x86_64/kvm · 24 cpus · 128320 MiB"
        );
    }

    #[test]
    fn a_machine_body_is_taken_as_the_daemon_sends_it() {
        // The daemon fills every field; a body missing one is a parse error
        // rather than a machine with invented values.
        let body = r#"{"name":"pi","arch":"aarch64","backend":"kvm"}"#;
        assert!(serde_json::from_str::<Machine>(body).is_err());
    }

    /// The frozen `/status` body, byte for byte as the live hub serves it
    /// (`vsockd --status-port 19080`, with a keepalive guest connected). The
    /// contract is this shape, so the client is tested against this shape.
    const HUB_STATUS: &str = concat!(
        r#"{"node":{"cids":[10,11,12,13,14,15],"port":5000,"uptime_s":673,"#,
        r#""max_sessions":64,"keepalive_idle_s":300},"#,
        r#""agents":[{"cid":10,"role":"builder","connected_at_ms":1760000000000,"#,
        r#""last_heartbeat_ms":1760000009000,"calls":7}],"sessions_active":1}"#
    );

    /// The same surface with nobody connected: the live hub's own answer on a
    /// host where no keepalive guest has connected yet.
    const HUB_STATUS_EMPTY: &str = concat!(
        r#"{"node":{"cids":[10,11,12,13,14,15],"port":5000,"uptime_s":673,"#,
        r#""max_sessions":64,"keepalive_idle_s":300},"agents":[],"sessions_active":0}"#
    );

    #[test]
    fn the_frozen_hub_status_parses_field_for_field() {
        let status: HubStatus = from_body(HUB_STATUS);
        assert_eq!(status.node.cids, vec![10, 11, 12, 13, 14, 15]);
        assert_eq!(status.node.port, 5_000);
        assert_eq!(status.node.uptime_s, 673);
        assert_eq!(status.node.max_sessions, 64);
        assert_eq!(status.node.keepalive_idle_s, 300);
        assert_eq!(status.sessions_active, 1);
        assert_eq!(status.uptime_ms(), 673_000);

        let agent = &status.agents[0];
        assert_eq!(agent.cid, 10);
        assert_eq!(agent.role, "builder");
        assert_eq!(agent.connected_at_ms, 1_760_000_000_000);
        assert_eq!(agent.last_heartbeat_ms, 1_760_000_009_000);
        assert_eq!(agent.calls, 7);
        assert_eq!(agent.heartbeat_age_ms(1_760_000_012_000), 3_000);
        assert_eq!(agent.connected_for_ms(1_760_000_012_000), 12_000);
        assert_eq!(
            agent.heartbeat_age_ms(1),
            0,
            "a clock behind the hub's is not a negative age"
        );

        // The join key: the hub's row for a CID, and nothing for a CID it does
        // not hold.
        assert_eq!(status.agent(10).map(|agent| agent.role.as_str()), Some("builder"));
        assert_eq!(status.agent(11), None);

        // The hub with nobody connected — the live hub's own observation.
        let empty: HubStatus = from_body(HUB_STATUS_EMPTY);
        assert!(empty.agents.is_empty());
        assert_eq!(empty.sessions_active, 0);
        assert_eq!(empty.node.cids.len(), 6);
    }

    #[test]
    fn a_hub_status_field_the_client_does_not_know_is_ignored() {
        // A newer hub may add fields without breaking this console: the known
        // shape parses, at every level of the body.
        let status: HubStatus = from_body(
            r#"{"node":{"cids":[10],"port":5000,"uptime_s":7,"max_sessions":64,
                "keepalive_idle_s":300,"future":"something"},
                "agents":[{"cid":10,"role":"builder","connected_at_ms":1,
                "last_heartbeat_ms":2,"calls":3,"latency_ms":4}],
                "sessions_active":1,"revision":"v2"}"#,
        );
        assert_eq!(status.node.port, 5_000);
        assert_eq!(status.agents[0].calls, 3);
        assert_eq!(status.sessions_active, 1);
    }

    #[test]
    fn a_hub_status_missing_a_field_is_an_error_that_names_it() {
        // Every level: the body, the node facts, an agent row.
        for (body, field) in [
            (
                r#"{"agents":[],"sessions_active":0}"#,
                "node",
            ),
            (
                r#"{"node":{"cids":[10],"port":5000,"uptime_s":7,"max_sessions":64},
                    "agents":[],"sessions_active":0}"#,
                "keepalive_idle_s",
            ),
            (
                r#"{"node":{"cids":[10],"port":5000,"uptime_s":7,"max_sessions":64,
                    "keepalive_idle_s":300},
                    "agents":[{"cid":10,"role":"builder","connected_at_ms":1,
                    "last_heartbeat_ms":2}],
                    "sessions_active":1}"#,
                "calls",
            ),
            (
                r#"{"node":{"cids":[10],"port":5000,"uptime_s":7,"max_sessions":64,
                    "keepalive_idle_s":300},"agents":[]}"#,
                "sessions_active",
            ),
        ] {
            let error = serde_json::from_str::<HubStatus>(body)
                .expect_err("a missing field must be refused");
            assert!(
                error.to_string().contains(field),
                "the error should name {field:?}: {error}"
            );
        }
    }

    #[test]
    fn a_hub_url_needs_its_port_and_stays_on_the_loopback() {
        assert_eq!(Endpoint::hub_url("127.0.0.1:19080").expect("loopback").port(), 19_080);
        assert_eq!(
            Endpoint::hub_url("http://localhost:19080/").expect("loopback").host(),
            "127.0.0.1"
        );
        // The status surface has no default port, so a bare host is refused by
        // name rather than pointed at the daemon's port.
        let error = Endpoint::hub_url("127.0.0.1").expect_err("no port to read");
        assert!(error.to_string().contains("--status-port"), "{error}");
    }

    #[test]
    fn the_hub_admin_url_holds_the_same_loopback_policy_and_names_its_own_flag() {
        assert_eq!(
            Endpoint::hub_admin_url("127.0.0.1:19081").expect("loopback").port(),
            19_081
        );
        assert_eq!(
            Endpoint::hub_admin_url("http://localhost:19081/").expect("loopback").host(),
            "127.0.0.1"
        );
        // The admin surface is off unless `--admin-port` asked for it, so a
        // bare host is refused by the flag that turns it on — never pointed at
        // the status port, the daemon's port, or anything else.
        let error = Endpoint::hub_admin_url("127.0.0.1").expect_err("no port to reach");
        assert!(error.to_string().contains("--admin-port"), "{error}");

        // The same exact-loopback negatives as the daemon and status urls: no
        // suffix, prefix, case or IP lookalike is a loopback host.
        for url in [
            "http://10.0.0.9:19081",
            "http://127.0.0.1.evil.tld:19081",
            "api.example.com:19081",
            "127.0.0.1.",
            "LOCALHOST:19081",
            "http://[::1]:19081",
            "0.0.0.0:19081",
            "https://127.0.0.1:19081",
            "127.0.0.1:19081/tell",
            "127.0.0.1:0",
        ] {
            let error = Endpoint::hub_admin_url(url)
                .expect_err("must be refused");
            assert!(
                matches!(error, ApiError::Config(_)),
                "{url} should be a configuration error, got {error:?}"
            );
        }
    }

    #[test]
    fn the_frozen_tell_answer_parses_and_a_missing_field_is_named() {
        let answer: TellAnswer =
            from_body(r#"{"ok":true,"output":"3.20 3.13 1.00","cid":300}"#);
        assert!(answer.ok);
        assert_eq!(answer.output, "3.20 3.13 1.00");

        // A declining tool is an answer too: `ok:false` with its reason.
        let declined: TellAnswer = from_body(r#"{"ok":false,"output":"denied: no such file"}"#);
        assert!(!declined.ok);
        assert_eq!(declined.output, "denied: no such file");

        // A missing field is an error that names it, at either position.
        for (body, field) in [
            (r#"{"output":"x"}"#, "ok"),
            (r#"{"ok":true}"#, "output"),
        ] {
            let error = serde_json::from_str::<TellAnswer>(body)
                .expect_err("a missing field must be refused");
            assert!(error.to_string().contains(field), "should name {field:?}: {error}");
        }
    }

    #[test]
    fn a_tell_refusal_carries_the_hubs_own_role_policy_words() {
        // The 403 the hub answers a role that does not grant the tool: its
        // tell body, not an `{"error": ...}` — and the message is the words it
        // wrote, not the JSON wrapper.
        let reply = Reply {
            status: 403,
            body: r#"{"ok":false,"output":"role librarian does not grant list"}"#.to_string(),
        };
        let error = reply.into_error();
        assert_eq!(error.status(), Some(403));
        assert_eq!(error.to_string(), "403: role librarian does not grant list");

        // Every other refusal is the hub's `{"error": ...}`, carried through.
        for (status, body, message) in [
            (404, r#"{"error":"no agent with guest CID 99"}"#, "no agent with guest CID 99"),
            (
                409,
                r#"{"error":"agent with guest CID 300 is not connected"}"#,
                "agent with guest CID 300 is not connected",
            ),
            (504, r#"{"error":"agent did not answer within 1 ms"}"#, "agent did not answer within 1 ms"),
            (400, r#"{"error":"a tell needs a tool name"}"#, "a tell needs a tool name"),
        ] {
            let error = Reply {
                status,
                body: body.to_string(),
            }
            .into_error();
            assert_eq!(error.status(), Some(status));
            assert_eq!(error.to_string(), format!("{status}: {message}"));
        }
    }

    #[test]
    fn a_stop_answer_parses_both_outcomes() {
        let stopped: StopAnswer =
            from_body(r#"{"cid":300,"stopped":true,"calls":3}"#);
        assert_eq!(
            stopped.into_outcome().expect("an outcome"),
            StopOutcome::Stopped { calls: 3 }
        );

        // A lapsed wait is a `200` too: nothing was torn down, and the hub says
        // why.
        let up: StopAnswer = from_body(
            r#"{"cid":300,"stopped":false,"reason":"the agent did not end within 5000 ms"}"#,
        );
        assert_eq!(
            up.into_outcome().expect("an outcome"),
            StopOutcome::StillUp {
                reason: "the agent did not end within 5000 ms".to_string(),
            }
        );
    }

    #[test]
    fn a_stop_answer_missing_its_branchs_field_is_named() {
        // `stopped` is required: a body without it is not a stop answer.
        assert!(serde_json::from_str::<StopAnswer>(r#"{"cid":300,"calls":3}"#).is_err());

        // The branch's own field: `calls` when stopped, `reason` when not.
        let ended = from_body::<StopAnswer>(r#"{"cid":300,"stopped":true}"#);
        let error = ended.into_outcome().expect_err("no count to read");
        assert!(matches!(error, ApiError::Malformed(_)), "{error:?}");
        assert!(error.to_string().contains("calls"), "{error}");

        let up = from_body::<StopAnswer>(r#"{"cid":300,"stopped":false}"#);
        let error = up.into_outcome().expect_err("no reason to read");
        assert!(error.to_string().contains("reason"), "{error}");
    }

    #[test]
    fn an_input_body_carries_exactly_the_daemons_fields() {
        let request = InputRequest {
            data: "uname\n".to_string(),
            vm: 1,
        };
        let body = serde_json::to_value(&request).expect("serializable");
        assert_eq!(body, serde_json::json!({ "data": "uname\n", "vm": 1 }));
        assert_eq!(
            serde_json::to_string(&request).expect("serializable"),
            r#"{"data":"uname\n","vm":1}"#
        );
        let parsed: InputRequest = from_body(&body.to_string());
        assert_eq!(parsed, request);
    }

    #[test]
    fn an_input_id_that_is_not_plain_is_refused_before_it_reaches_a_path() {
        assert!(Client::default().input("r1/../r2", 0, "uname\n").is_err());
    }

    /// A tasks body as `GET /api/v0/tasks` sends it, byte for byte the shape
    /// the live daemon answered with: a claimed task with a note next to one
    /// nobody has taken.
    const TASKS: &str = r#"{"tasks":[
        {"id":"t1","goal":"write the plaza handbooks","acceptance":"all 21 present",
         "budget":2000,"role":"librarian","state":"done","outcome":"accepted",
         "assignee_id":"librarian","verifier_id":null,"agent_cid":112,
         "note":"21 handbooks committed","created_at":1791671813178,
         "updated_at":1791671813568},
        {"id":"t2","goal":"port the ledger to the console","acceptance":null,
         "budget":0,"role":null,"state":"open","outcome":null,"assignee_id":null,
         "verifier_id":null,"agent_cid":null,"note":null,"created_at":1791671900000,
         "updated_at":1791671900000}
    ]}"#;

    #[test]
    fn a_tasks_body_parses_every_column_the_daemon_writes() {
        let page: TasksPage = from_body(TASKS);
        assert_eq!(page.tasks.len(), 2);

        let claimed = &page.tasks[0];
        assert_eq!(claimed.id, "t1");
        assert_eq!(claimed.goal, "write the plaza handbooks");
        assert_eq!(claimed.acceptance.as_deref(), Some("all 21 present"));
        assert_eq!(claimed.budget, 2_000);
        assert_eq!(claimed.role.as_deref(), Some("librarian"));
        assert_eq!(claimed.state, TaskState::Done);
        assert_eq!(claimed.outcome.get(), Some(&TaskOutcome::Accepted));
        assert_eq!(claimed.assignee_id.as_deref(), Some("librarian"));
        assert_eq!(claimed.verifier_id.get(), None, "v0 records no verifier");
        assert_eq!(claimed.agent_cid.copied(), Some(112));
        assert_eq!(claimed.note.as_deref(), Some("21 handbooks committed"));
        assert_eq!(claimed.created_at, 1_791_671_813_178);
        assert_eq!(claimed.updated_at, 1_791_671_813_568);
        assert!(claimed.state.is_finished());

        let open = &page.tasks[1];
        assert_eq!(open.state, TaskState::Open);
        assert!(!open.state.is_finished());
        assert!(open.state.is_claimable());
        assert!(!open.state.is_finishable());
        assert_eq!(open.acceptance.get(), None, "a null column is none");
        assert_eq!(open.outcome.get(), None, "a living task has no outcome");
        assert_eq!(open.assignee_id.get(), None);
        assert_eq!(open.agent_cid.get(), None);
        assert_eq!(open.note.get(), None);
        assert_eq!(open.role.get(), None);
    }

    /// A whole task body — every column the daemon writes — with the named
    /// ones left out: what a body that is not the daemon's shape looks like.
    fn body_without(missing: &[&str]) -> String {
        let mut body = serde_json::json!({
            "id": "t1", "goal": "g", "acceptance": null, "budget": 0, "role": null,
            "state": "open", "outcome": null, "assignee_id": null,
            "verifier_id": null, "agent_cid": null, "note": null,
            "created_at": 1, "updated_at": 1,
        });
        for column in missing {
            body.as_object_mut()
                .expect("the body is an object")
                .remove(*column);
        }
        body.to_string()
    }

    #[test]
    fn a_task_missing_a_column_is_an_error_that_names_it() {
        // Every column the daemon writes is required — the nullable ones
        // included: `null` and "absent" are not the same fact, and a body that
        // omits one is not the shape this client reads. The whole body is
        // walked so no column is quietly optional.
        for column in [
            "id",
            "goal",
            "acceptance",
            "budget",
            "role",
            "state",
            "outcome",
            "assignee_id",
            "verifier_id",
            "agent_cid",
            "note",
            "created_at",
            "updated_at",
        ] {
            let body = body_without(&[column]);
            let error = serde_json::from_str::<Task>(&body)
                .expect_err("a body missing a column must be refused");
            assert!(
                error.to_string().contains(column),
                "the error should name {column:?}: {error}"
            );
        }
        // The whole shape, and none of it missing, parses.
        let task: Task = from_body(&body_without(&[]));
        assert_eq!(task.id, "t1");
        assert_eq!(task.state, TaskState::Open);
    }

    #[test]
    fn a_task_column_the_client_does_not_know_is_ignored() {
        // A newer daemon may add a column without breaking the console.
        let task: Task = from_body(
            r#"{"id":"t1","goal":"g","acceptance":null,"budget":7,"role":null,
                "state":"claimed","outcome":null,"assignee_id":"builder",
                "verifier_id":null,"agent_cid":100,"note":null,"created_at":1,
                "updated_at":2,"parent_id":"t0","change_sets":[]}"#,
        );
        assert_eq!(task.id, "t1");
        assert_eq!(task.state, TaskState::Claimed);
        assert_eq!(task.assignee_id.as_deref(), Some("builder"));
        assert_eq!(task.agent_cid.copied(), Some(100));
    }

    #[test]
    fn a_task_state_or_outcome_the_client_does_not_know_parses() {
        let task: Task = from_body(
            r#"{"id":"t9","goal":"g","acceptance":null,"budget":0,"role":null,
                "state":"verifying","outcome":"reworked","assignee_id":null,
                "verifier_id":null,"agent_cid":null,"note":null,"created_at":1,
                "updated_at":1}"#,
        );
        assert_eq!(task.state, TaskState::Unknown);
        assert_eq!(task.outcome.get(), Some(&TaskOutcome::Unknown));
        assert!(!task.state.is_finished());
        assert!(!task.state.is_claimable());
        assert!(!task.state.is_withdrawable());
    }

    /// An artifact body as `POST /api/v0/tasks/{id}/artifact` answers it, byte
    /// for byte the shape the live daemon answered with.
    const ARTIFACT: &str = r#"{"task":"t3","agent_cid":103,"path":"build-notes.txt",
        "content":"task t3: verify the fixed loop","size":30,"truncated":false}"#;

    #[test]
    fn an_artifact_body_parses_every_column_the_daemon_writes() {
        let artifact: TaskArtifact = from_body(ARTIFACT);
        assert_eq!(artifact.task, "t3");
        assert_eq!(artifact.agent_cid, 103);
        assert_eq!(artifact.path, "build-notes.txt");
        assert_eq!(artifact.content, "task t3: verify the fixed loop");
        assert_eq!(artifact.size, 30);
        assert!(!artifact.truncated);
    }

    #[test]
    fn an_artifact_missing_a_column_is_an_error_that_names_it() {
        // None of the artifact's columns is nullable, so every one of them is
        // required: a body that omits one is not the shape this client reads.
        for column in ["task", "agent_cid", "path", "content", "size", "truncated"] {
            let mut body: serde_json::Value = from_body(ARTIFACT);
            body.as_object_mut()
                .expect("the body is an object")
                .remove(column);
            let error = serde_json::from_str::<TaskArtifact>(&body.to_string())
                .expect_err("a body missing a column must be refused");
            assert!(
                error.to_string().contains(column),
                "the error should name {column:?}: {error}"
            );
        }
    }

    #[test]
    fn an_artifact_column_the_client_does_not_know_is_ignored() {
        let artifact: TaskArtifact = from_body(
            r#"{"task":"t3","agent_cid":103,"path":"n.txt","content":"hi","size":2,
                "truncated":true,"mime":"text/plain","blob_hash":"abc"}"#,
        );
        assert_eq!(artifact.content, "hi");
        assert!(artifact.truncated);
    }

    #[test]
    fn an_artifact_request_body_carries_exactly_the_path() {
        let request = ArtifactRequest {
            path: "build-notes.txt".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&request).expect("serializable"),
            r#"{"path":"build-notes.txt"}"#
        );
    }

    #[test]
    fn an_artifact_task_id_that_is_not_plain_is_refused_before_it_reaches_a_path() {
        for id in ["", "t1/../t2", "t1?x=1", "t1 t2"] {
            assert!(
                matches!(
                    Client::default().task_artifact(id, "notes.txt"),
                    Err(ApiError::Config(_))
                ),
                "{id:?} should never reach a path"
            );
        }
    }

    #[test]
    fn the_states_and_outcomes_are_the_daemons_own_words() {
        for (state, label, stamp) in [
            (TaskState::Open, "open", "OPEN"),
            (TaskState::Claimed, "claimed", "CLAIMED"),
            (TaskState::Done, "done", "DONE"),
            (TaskState::Failed, "failed", "FAILED"),
            (TaskState::Withdrawn, "withdrawn", "WITHDRAWN"),
            (TaskState::Unknown, "unknown", "UNKNOWN"),
        ] {
            assert_eq!(state.label(), label);
            assert_eq!(state.stamp_label(), stamp);
        }
        // The daemon's four moves, as guards: a claim only from `open`, a
        // finish only from `claimed`, a withdraw from either.
        assert!(TaskState::Open.is_claimable() && !TaskState::Claimed.is_claimable());
        assert!(TaskState::Claimed.is_finishable() && !TaskState::Open.is_finishable());
        assert!(TaskState::Open.is_withdrawable() && TaskState::Claimed.is_withdrawable());
        assert!(!TaskState::Done.is_withdrawable() && !TaskState::Failed.is_withdrawable());
        assert_eq!(TaskOutcome::Accepted.label(), "accepted");
        assert_eq!(TaskOutcome::Failed.label(), "failed");
        assert_eq!(TaskOutcome::Cancelled.label(), "cancelled");
        assert_eq!(FinishOutcome::Done.label(), "done");
        assert_eq!(FinishOutcome::Failed.label(), "failed");
    }

    #[test]
    fn a_create_task_body_carries_exactly_the_daemons_fields() {
        let create = CreateTask {
            goal: "ship the ledger".to_string(),
            acceptance: Some("the pane draws it".to_string()),
            budget: Some(50),
            role: None,
        };
        let body = serde_json::to_value(&create).expect("serializable");
        assert_eq!(
            body,
            serde_json::json!({
                "goal": "ship the ledger",
                "acceptance": "the pane draws it",
                "budget": 50,
                "role": null
            })
        );
        let parsed: CreateTask = from_body(&body.to_string());
        assert_eq!(parsed, create);
    }

    #[test]
    fn a_claim_and_a_finish_body_are_exactly_the_daemons() {
        assert_eq!(
            serde_json::to_string(&ClaimTask { agent_cid: 112 }).expect("serializable"),
            r#"{"agent_cid":112}"#
        );
        assert_eq!(
            serde_json::to_string(&FinishTask {
                outcome: FinishOutcome::Failed,
                note: Some("the guest never answered".to_string()),
            })
            .expect("serializable"),
            r#"{"outcome":"failed","note":"the guest never answered"}"#
        );
        // A finish with no note is still the whole request shape.
        assert_eq!(
            serde_json::to_string(&FinishTask {
                outcome: FinishOutcome::Done,
                note: None,
            })
            .expect("serializable"),
            r#"{"outcome":"done","note":null}"#
        );
        let parsed: FinishTask =
            from_body(r#"{"outcome":"done","note":null}"#);
        assert_eq!(parsed.outcome, FinishOutcome::Done);
        assert_eq!(parsed.note, None);
    }

    #[test]
    fn a_task_id_that_is_not_plain_is_refused_before_it_reaches_a_path() {
        let client = Client::default();
        for id in ["", "t1/../t2", "t1?x=1", "t1 t2", "t\u{00e9}"] {
            assert!(matches!(client.task(id), Err(ApiError::Config(_))), "{id:?}");
        }
        assert!(client.claim_task("t1/claim", 4).is_err());
        assert!(client.finish_task("t1/../t2", FinishOutcome::Done, None).is_err());
        assert!(client.withdraw_task("t1/../t2").is_err());
    }

    /// A treasury body as `GET /api/v0/treasury` sends it, byte for byte the
    /// shape the live daemon answered with after one budgeted task's cycle:
    /// the mint account and the task's escrow, balanced to zero.
    const TREASURY: &str = r#"{"accounts":[
        {"id":1,"kind":"mint","owner_ref":"mint","credit_limit":0,"balance":0,
         "created_at":1791680742762},
        {"id":2,"kind":"task_escrow","owner_ref":"t1","credit_limit":0,"balance":0,
         "created_at":1791680766728}],
        "totals":{"accounts":2,"entries":2,"credits":1500,"debits":1500,"balance":0},
        "spent":0,"note":"v0 meters no real spend; a done task refunds its whole escrow"}"#;

    /// A journal body as `GET /api/v0/treasury/journal` sends it: the one
    /// cycle's two entries, each with its postings.
    const JOURNAL: &str = r#"{"entries":[
        {"id":1,"kind":"mint","task_id":"t1","root_task":"t1","memo":"task budget",
         "posted_by":"treasury-service","created_at":1791680766728,"amount":750,
         "postings":[{"account":1,"kind":"mint","owner_ref":"mint","amount":-750},
                     {"account":2,"kind":"task_escrow","owner_ref":"t1","amount":750}]},
        {"id":2,"kind":"refund","task_id":"t1","root_task":"t1","memo":"task budget refunded",
         "posted_by":"treasury-service","created_at":1791680768447,"amount":750,
         "postings":[{"account":2,"kind":"task_escrow","owner_ref":"t1","amount":-750},
                     {"account":1,"kind":"mint","owner_ref":"mint","amount":750}]}],
        "total":2,"limit":256,"truncated":false}"#;

    #[test]
    fn a_treasury_body_parses_every_column_the_daemon_writes() {
        let treasury: Treasury = from_body(TREASURY);
        assert_eq!(treasury.accounts.len(), 2);

        let mint = &treasury.accounts[0];
        assert_eq!(mint.id, 1);
        assert_eq!(mint.kind, AccountKind::Mint);
        assert_eq!(mint.owner_ref, "mint");
        assert_eq!(mint.credit_limit, 0);
        assert_eq!(mint.balance, 0);
        assert_eq!(mint.created_at, 1_791_680_742_762);

        let escrow = &treasury.accounts[1];
        assert_eq!(escrow.kind, AccountKind::TaskEscrow);
        assert_eq!(escrow.kind.label(), "task_escrow");
        assert_eq!(escrow.owner_ref, "t1");

        assert_eq!(treasury.totals.accounts, 2);
        assert_eq!(treasury.totals.entries, 2);
        assert_eq!(treasury.totals.credits, 1_500);
        assert_eq!(treasury.totals.debits, 1_500);
        assert_eq!(treasury.totals.balance, 0, "the journal balances");

        assert_eq!(treasury.spent, 0, "v0 meters no real spend");
        assert_eq!(
            treasury.note,
            "v0 meters no real spend; a done task refunds its whole escrow",
            "the daemon's own note, verbatim"
        );
    }

    /// The whole treasury body as JSON, with the named paths removed: what a
    /// body that is not the daemon's shape looks like. A path with no `/` is a
    /// top-level column; `accounts/<field>` and `totals/<field>` are the
    /// nested ones.
    fn treasury_without(remove: &[&str]) -> String {
        let mut body: serde_json::Value = from_body(TREASURY);
        for path in remove {
            match path.split_once('/') {
                Some(("accounts", field)) => {
                    body["accounts"][0]
                        .as_object_mut()
                        .expect("an accounts row is an object")
                        .remove(field);
                }
                Some(("totals", field)) => {
                    body["totals"]
                        .as_object_mut()
                        .expect("the totals are an object")
                        .remove(field);
                }
                Some(other) => panic!("the fixture has no path {other:?}"),
                None => {
                    body.as_object_mut()
                        .expect("the body is an object")
                        .remove(*path);
                }
            }
        }
        body.to_string()
    }

    #[test]
    fn a_treasury_missing_a_column_is_an_error_that_names_it() {
        // Every column the daemon writes is required, at every level — none of
        // the treasury's own is nullable. The whole body is walked so no
        // column is quietly optional.
        for path in [
            "accounts",
            "totals",
            "spent",
            "note",
            "accounts/id",
            "accounts/kind",
            "accounts/owner_ref",
            "accounts/credit_limit",
            "accounts/balance",
            "accounts/created_at",
            "totals/accounts",
            "totals/entries",
            "totals/credits",
            "totals/debits",
            "totals/balance",
        ] {
            let body = treasury_without(&[path]);
            let error = serde_json::from_str::<Treasury>(&body)
                .expect_err("a body missing a column must be refused");
            let named = path.rsplit('/').next().expect("a field name");
            assert!(
                error.to_string().contains(named),
                "the error should name {named:?} for {path:?}: {error}"
            );
        }
        // The whole shape, and none of it missing, parses.
        let treasury: Treasury = from_body(&treasury_without(&[]));
        assert_eq!(treasury.accounts.len(), 2);
    }

    #[test]
    fn a_treasury_kind_the_client_does_not_know_parses_as_unknown() {
        let mut body: serde_json::Value = from_body(TREASURY);
        body["accounts"][0]["kind"] = serde_json::json!("escrow_of_the_moon");
        let treasury: Treasury = from_body(&body.to_string());
        assert_eq!(treasury.accounts[0].kind, AccountKind::Unknown);
        assert_eq!(treasury.accounts[0].kind.label(), "unknown");
    }

    #[test]
    fn a_treasury_column_the_client_does_not_know_is_ignored() {
        // A newer daemon may add a column without breaking the console.
        let mut body: serde_json::Value = from_body(TREASURY);
        body["accounts"][0]["frozen"] = serde_json::json!(false);
        body["unsealed_total"] = serde_json::json!(0);
        let treasury: Treasury = from_body(&body.to_string());
        assert_eq!(treasury.accounts[0].id, 1);
        assert_eq!(treasury.spent, 0);
    }

    #[test]
    fn a_journal_body_parses_every_column_the_daemon_writes() {
        let journal: Journal = from_body(JOURNAL);
        assert_eq!(journal.entries.len(), 2);
        assert_eq!(journal.total, 2);
        assert_eq!(journal.limit, 256);
        assert!(!journal.truncated);

        let mint = &journal.entries[0];
        assert_eq!(mint.id, 1);
        assert_eq!(mint.kind, EntryKind::Mint);
        assert_eq!(mint.kind.label(), "mint");
        assert_eq!(mint.task_id.as_deref(), Some("t1"));
        assert_eq!(mint.root_task.as_deref(), Some("t1"));
        assert_eq!(mint.memo, "task budget");
        assert_eq!(mint.posted_by, "treasury-service");
        assert_eq!(mint.created_at, 1_791_680_766_728);
        assert_eq!(mint.amount, 750);
        assert_eq!(mint.postings.len(), 2);

        // The postings: the mint's debit leaves the mint, its credit fills the
        // escrow — the sign *is* the side.
        let debit = &mint.postings[0];
        assert_eq!(debit.account, 1);
        assert_eq!(debit.kind, AccountKind::Mint);
        assert_eq!(debit.owner_ref, "mint");
        assert_eq!(debit.amount, -750);
        assert!(debit.is_debit());
        assert_eq!(debit.magnitude(), 750);

        let credit = &mint.postings[1];
        assert_eq!(credit.kind, AccountKind::TaskEscrow);
        assert_eq!(credit.owner_ref, "t1");
        assert_eq!(credit.amount, 750);
        assert!(!credit.is_debit());
        assert_eq!(credit.magnitude(), 750);

        // Every entry balances: its credits equal its debits.
        for entry in &journal.entries {
            let credits: i64 = entry.postings.iter().filter(|p| !p.is_debit()).map(Posting::magnitude).sum();
            let debits: i64 = entry.postings.iter().filter(|p| p.is_debit()).map(Posting::magnitude).sum();
            assert_eq!(credits, debits, "entry {} balances", entry.id);
            assert_eq!(i128::from(credits), entry.amount, "entry {}'s size is its credited total", entry.id);
        }
    }

    /// The journal body with one column of its first entry removed.
    fn journal_without_entry_field(field: &str) -> String {
        let mut body: serde_json::Value = from_body(JOURNAL);
        body["entries"][0]
            .as_object_mut()
            .expect("an entry is an object")
            .remove(field);
        body.to_string()
    }

    #[test]
    fn a_journal_missing_a_column_is_an_error_that_names_it() {
        for path in ["entries", "total", "limit", "truncated"] {
            let mut body: serde_json::Value = from_body(JOURNAL);
            body.as_object_mut()
                .expect("the body is an object")
                .remove(path);
            let error = serde_json::from_str::<Journal>(&body.to_string())
                .expect_err("a body missing a column must be refused");
            assert!(
                error.to_string().contains(path),
                "the error should name {path:?}: {error}"
            );
        }
        for field in [
            "id",
            "kind",
            "task_id",
            "root_task",
            "memo",
            "posted_by",
            "created_at",
            "amount",
            "postings",
        ] {
            let error = serde_json::from_str::<Journal>(&journal_without_entry_field(field))
                .expect_err("an entry missing a column must be refused");
            assert!(
                error.to_string().contains(field),
                "the error should name {field:?}: {error}"
            );
        }
        // A posting's own columns are required too.
        for field in ["account", "kind", "owner_ref", "amount"] {
            let mut body: serde_json::Value = from_body(JOURNAL);
            body["entries"][0]["postings"][0]
                .as_object_mut()
                .expect("a posting is an object")
                .remove(field);
            let error = serde_json::from_str::<Journal>(&body.to_string())
                .expect_err("a posting missing a column must be refused");
            assert!(
                error.to_string().contains(field),
                "the error should name {field:?}: {error}"
            );
        }
    }

    #[test]
    fn a_journal_task_column_is_nullable_but_never_absent() {
        // An entry that belongs to no task (a `fine` or a `fee`) writes
        // `null`, which reads as none rather than as an error.
        let journal: Journal = from_body(
            r#"{"entries":[{"id":7,"kind":"fine","task_id":null,"root_task":null,
                 "memo":"a sanction","posted_by":"treasury-service","created_at":1,
                 "amount":5,"postings":[{"account":3,"kind":"agent_earnings",
                 "owner_ref":"cid:100","amount":-5},{"account":4,"kind":"treasury",
                 "owner_ref":"treasury","amount":5}]}],
                "total":1,"limit":256,"truncated":false}"#,
        );
        assert_eq!(journal.entries[0].task_id.get(), None);
        assert_eq!(journal.entries[0].root_task.get(), None);

        // Absent is a different fact from null, and it is named.
        let mut body: serde_json::Value = from_body(JOURNAL);
        body["entries"][0]
            .as_object_mut()
            .expect("an object")
            .remove("task_id");
        let error = serde_json::from_str::<Journal>(&body.to_string())
            .expect_err("an absent column is not a null one");
        assert!(error.to_string().contains("task_id"), "{error}");
    }

    #[test]
    fn a_journal_entry_kind_the_client_does_not_know_parses_as_unknown() {
        let mut body: serde_json::Value = from_body(JOURNAL);
        body["entries"][0]["kind"] = serde_json::json!("tithe");
        let journal: Journal = from_body(&body.to_string());
        assert_eq!(journal.entries[0].kind, EntryKind::Unknown);
        assert_eq!(journal.entries[0].kind.label(), "unknown");
    }

    #[test]
    fn a_journal_that_was_cut_to_its_limit_says_so() {
        let mut body: serde_json::Value = from_body(JOURNAL);
        body["total"] = serde_json::json!(900);
        body["truncated"] = serde_json::json!(true);
        let journal: Journal = from_body(&body.to_string());
        assert_eq!(journal.entries.len(), 2, "the response carries what it carries");
        assert_eq!(journal.total, 900);
        assert!(journal.truncated);
    }

    #[test]
    fn the_treasury_read_is_both_halves_or_neither() {
        // The read is the pane's own unit: a treasury view and a journal.
        let read = TreasuryRead {
            treasury: from_body(TREASURY),
            journal: from_body(JOURNAL),
        };
        assert_eq!(read.treasury.totals.entries, read.journal.total);
        assert_eq!(read.treasury.spent, 0);
    }
}
