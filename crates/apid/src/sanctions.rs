//! Sanctions for the plaza's law violations, by the schedule in `docs/codex-draft/sanctions.yaml`.
//!
//! `laws.rs` records a violation and hands it to the violation sink, which `db::wire_violations`
//! chains to [`on_violation`]. The schedule picks the steps by the law's severity and the agent's
//! offence count (confirmed violations of the same law in the last 30 days, this one included).
//! Code applies them, never the agent:
//!
//! - `warning`: a record on the agent's profile, a note in its memory and a `sanction` bus event.
//! - `fine:N`: a `Fine` entry from the agent's earnings to the treasury. Capped by `max_fine` and by
//!   [`WEEKLY_FINE_CAP`] over seven days; a fine the wallet cannot pay is recorded as not collected.
//! - `probation:D` and `lockout:D`: the offending tool is suspended for D (the gate refuses it, see
//!   [`suspended`]). A lockout also asks the owner for an approval.
//! - `demotion`, `retirement`, `owner-review`: the owner is asked for an approval. apid does not carry
//!   them out.
//!
//! The owner answers the approval with [`decided`]: approve acknowledges the sanction, deny lifts an
//! active suspension, and the action `retire` is only recorded. The owner may also lift a suspension
//! at any time through `POST /v1/sanctions/{id}/lift` (see [`routes`]).
//!
//! Classification is always `negligent` (the schedule as written): no law carries a
//! classification annotation yet, so an `induced` or `malicious` case cannot be told apart.
//!
//! The record is the `sanctions` component of the database (`db::COMPONENTS`). `<state-dir>/sanctions.json`
//! is read once, for migration, when the database holds no such component. The file is also written on
//! every change, so a run without a database keeps its record there.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, Once, OnceLock, PoisonError};

use apz_http::{Params, Request, Response, Router};
use apz_plaza::treasury::Treasury;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::approvals::Approval;
use crate::events::Bus;
use crate::laws::Violation;
use crate::memory::Memory;
use crate::settings::{FINES_PER_WEEK_MAX, Sanctions};
use crate::state::App;

/// The most one agent may be fined in seven days (sanctions.yaml, `caps.max_fine_per_week`).
pub const WEEKLY_FINE_CAP: u64 = FINES_PER_WEEK_MAX;
/// Applied sanctions kept; the oldest go first.
const KEEP: usize = 1000;
/// The sanction name of a tool suspension's record.
const SUSPENSION: &str = "tool-suspension:";
/// Records `GET /v1/sanctions` returns when no limit is given.
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 10_000;

const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 24 * 60 * MINUTE_MS;
const WEEK_MS: u64 = 7 * DAY_MS;

/// The schedule, row by severity, column by offence count 1, 2 and 3 (3 and up use the third).
/// Each cell is a sanctions.yaml cell verbatim: steps separated by "; ".
const SCHEDULE: [(&str, [&str; 3]); 5] = [
    ("info", ["warning", "warning", "fine:50"]),
    ("low", ["warning", "fine:100", "fine:200; probation:3d"]),
    (
        "medium",
        [
            "warning; fine:100",
            "fine:300; probation:7d",
            "lockout:3d; demotion",
        ],
    ),
    (
        "high",
        [
            "fine:300; probation:7d",
            "lockout:7d; demotion",
            "retirement; owner-review",
        ],
    ),
    (
        "critical",
        [
            "lockout:14d; owner-review",
            "retirement; owner-review",
            "retirement; owner-review",
        ],
    ),
];

/// A law's severity on the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// The schedule's severity for a law's `severity` keyword. An unknown one is `info`, the mildest.
    pub fn of(law_severity: &str) -> Severity {
        match law_severity {
            "critical" => Severity::Critical,
            "high" => Severity::High,
            "medium" | "major" => Severity::Medium,
            "low" | "minor" => Severity::Low,
            _ => Severity::Info,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

/// One step of a sanction, as the schedule words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Warning,
    /// Credits to take from the agent's earnings, before the caps.
    Fine(u64),
    /// The offending tool is refused for `ms` (probation and lockout).
    Suspend {
        ms: u64,
        label: String,
    },
    /// Named by the schedule but not carried out here: the owner is asked.
    Escalate(String),
}

/// The steps a severity gets at an offence count (1, 2, and 3 for every count from 3 on).
pub fn steps(severity: Severity, count: usize) -> Vec<Step> {
    let column = count.clamp(1, 3) - 1;
    SCHEDULE
        .iter()
        .find(|(key, _)| *key == severity.key())
        .map_or_else(Vec::new, |(_, row)| {
            row[column].split("; ").flat_map(parse_step).collect()
        })
}

fn parse_step(text: &str) -> Vec<Step> {
    let (name, arg) = text.split_once(':').unwrap_or((text, ""));
    match name {
        "warning" => vec![Step::Warning],
        "fine" => match arg.parse() {
            Ok(credits) => vec![Step::Fine(credits)],
            Err(_) => vec![Step::Escalate(text.to_string())],
        },
        "probation" | "lockout" => match duration_ms(arg) {
            Some(ms) => vec![
                Step::Suspend {
                    ms,
                    label: arg.to_string(),
                },
                Step::Escalate(text.to_string()),
            ],
            None => vec![Step::Escalate(text.to_string())],
        },
        _ => vec![Step::Escalate(text.to_string())],
    }
}

/// `3d`, `12h` or `30m` as milliseconds.
fn duration_ms(text: &str) -> Option<u64> {
    let (digits, unit_ms) = if let Some(days) = text.strip_suffix('d') {
        (days, DAY_MS)
    } else if let Some(hours) = text.strip_suffix('h') {
        (hours, 60 * MINUTE_MS)
    } else {
        let minutes = text.strip_suffix('m')?;
        (minutes, MINUTE_MS)
    };
    digits.parse::<u64>().ok()?.checked_mul(unit_ms)
}

/// What one violation is sanctioned for.
pub struct Case<'a> {
    pub cid: u64,
    pub law: &'a str,
    pub title: &'a str,
    pub severity: Severity,
    /// Violations of this law by this agent in the window, this one included.
    pub count: usize,
    pub tool: &'a str,
    pub at_ms: u64,
}

/// Where the sanctions' effects go: the treasury, the bus, the agent's memory, and the owner's approvals.
pub struct Env<'a> {
    pub treasury: &'a Treasury,
    pub bus: &'a Bus,
    pub memory: Option<&'a Memory>,
    /// Queues an approval for the owner from `(kind, subject, detail)` and returns its id. `None` queues none.
    pub approvals: Option<&'a dyn Fn(&str, &str, Value) -> String>,
}

/// The owner's lift of a tool suspension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifted {
    pub at_ms: u64,
    /// Always "owner".
    pub by: String,
    pub why: String,
}

/// The owner's latest decision on a sanction record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerAction {
    /// "acknowledge" or "retire".
    pub action: String,
    pub at_ms: u64,
}

/// One sanction as applied (or not), kept on the agent's record and published as a `sanction` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// Sequential from the ledger's counter, which is saved with the record: ids are not reused.
    #[serde(default)]
    pub id: u64,
    pub cid: u64,
    pub law: String,
    pub title: String,
    pub severity: String,
    pub count: usize,
    pub tool: String,
    pub at_ms: u64,
    /// The step as the schedule words it: "warning", "fine:100", "tool-suspension:3d", "owner-review".
    pub sanction: String,
    /// "applied", "escalated", or "not collected: <why>".
    pub outcome: String,
    /// Credits taken from the wallet (0 unless a fine was collected).
    pub fine: u64,
    /// The treasury entry of a collected fine.
    pub entry: Option<u64>,
    /// When a tool suspension ends, in ms.
    pub until_ms: Option<u64>,
    /// The approval that asks the owner about this step (escalations only).
    #[serde(default)]
    pub approval: Option<String>,
    /// The owner's lift of a tool suspension.
    #[serde(default)]
    pub lifted: Option<Lifted>,
    /// The owner's latest decision on this record.
    #[serde(default)]
    pub owner_action: Option<OwnerAction>,
}

/// A tool an agent may not call until `until_ms`. Its `id` is the id of its tool-suspension record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspension {
    #[serde(default)]
    pub id: u64,
    pub cid: u64,
    pub tool: String,
    pub law: String,
    pub until_ms: u64,
    #[serde(default)]
    pub lifted: bool,
}

/// A suspension as the API shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuspensionView {
    pub id: u64,
    pub cid: u64,
    pub tool: String,
    pub law: String,
    pub until_ms: u64,
    /// Until `until_ms`, and not lifted.
    pub active: bool,
}

/// A stored suspension as the API shows it, at `now_ms`.
fn view(s: &Suspension, now_ms: u64) -> SuspensionView {
    SuspensionView {
        id: s.id,
        cid: s.cid,
        tool: s.tool.clone(),
        law: s.law.clone(),
        until_ms: s.until_ms,
        active: !s.lifted && s.until_ms > now_ms,
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    applied: Vec<Applied>,
    suspensions: Vec<Suspension>,
    /// The last id given out.
    next_id: u64,
}

impl State {
    /// Numbers the records of a snapshot from before ids (their id is 0), and keeps the counter past
    /// every id. A suspension without an id takes the id of its tool-suspension record.
    fn number(&mut self) {
        for record in self.applied.iter_mut().filter(|a| a.id == 0) {
            self.next_id += 1;
            record.id = self.next_id;
        }
        self.next_id = self
            .applied
            .iter()
            .map(|a| a.id)
            .max()
            .unwrap_or(0)
            .max(self.next_id);
        let (applied, next_id) = (&self.applied, &mut self.next_id);
        for suspension in self.suspensions.iter_mut().filter(|s| s.id == 0) {
            let from = applied.iter().find(|a| {
                a.sanction.starts_with(SUSPENSION)
                    && a.cid == suspension.cid
                    && a.tool == suspension.tool
                    && a.law == suspension.law
                    && a.until_ms == Some(suspension.until_ms)
            });
            suspension.id = match from {
                Some(record) => record.id,
                None => {
                    *next_id += 1;
                    *next_id
                }
            };
        }
    }
}

/// Why a sanction cannot be lifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// The variant names state the refusal; renaming them changes the public API.
#[allow(clippy::enum_variant_names)]
pub enum LiftError {
    NotFound,
    NotSuspension,
    NotActive,
}

impl LiftError {
    fn message(self, id: u64) -> String {
        match self {
            LiftError::NotFound => format!("no sanction {id}"),
            LiftError::NotSuspension => format!("sanction {id} is not a tool suspension"),
            LiftError::NotActive => format!("the suspension {id} is not active"),
        }
    }
}

/// The sanctions on record and the suspensions in force. One per apid process.
pub struct Ledger {
    /// `<state-dir>/sanctions.json`: read once for migration, written on every change.
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// One sanction at a time: the weekly fine cap is read and then spent.
    deciding: Mutex<()>,
}

impl Ledger {
    fn new(path: Option<PathBuf>) -> Ledger {
        Ledger {
            path,
            state: Mutex::new(State::default()),
            deciding: Mutex::new(()),
        }
    }

    /// Applies the schedule's steps for `case`. Nothing happens when sanctions are disabled.
    pub fn apply(&self, env: &Env, settings: &Sanctions, case: &Case) -> Vec<Applied> {
        if !settings.enabled {
            return Vec::new();
        }
        let _one_at_a_time = self.deciding.lock().unwrap_or_else(PoisonError::into_inner);
        let mut applied = Vec::new();
        // The end of this case's tool suspension, which a lockout escalation names.
        let mut suspended_until: Option<u64> = None;
        for step in steps(case.severity, case.count) {
            let record = match step {
                Step::Warning => {
                    note(env, case);
                    Some(self.new_record(case, "warning", "applied"))
                }
                Step::Fine(credits) => self.fine(env, settings, case, credits),
                Step::Suspend { ms, label } => {
                    let record = self.suspend(settings, case, ms, &label);
                    suspended_until = record.as_ref().and_then(|r| r.until_ms).or(suspended_until);
                    record
                }
                Step::Escalate(name) => Some(self.escalate(env, case, &name, suspended_until)),
            };
            applied.extend(record);
        }
        {
            let mut state = self.lock();
            state.applied.extend(applied.iter().cloned());
            let excess = state.applied.len().saturating_sub(KEEP);
            state.applied.drain(..excess);
            state
                .suspensions
                .retain(|suspension| suspension.until_ms > case.at_ms);
            self.save(&state);
        }
        for record in &applied {
            env.bus.publish("sanction", record);
        }
        applied
    }

    /// A record of `case`, numbered from the counter (saved with the state, so ids are not reused).
    fn new_record(&self, case: &Case, sanction: &str, outcome: &str) -> Applied {
        let id = {
            let mut state = self.lock();
            state.next_id += 1;
            state.next_id
        };
        Applied {
            id,
            cid: case.cid,
            law: case.law.to_string(),
            title: case.title.to_string(),
            severity: case.severity.key().to_string(),
            count: case.count,
            tool: case.tool.to_string(),
            at_ms: case.at_ms,
            sanction: sanction.to_string(),
            outcome: outcome.to_string(),
            fine: 0,
            entry: None,
            until_ms: None,
            approval: None,
            lifted: None,
            owner_action: None,
        }
    }

    /// A fine from the agent's earnings, within `max_fine` and the week's room under the cap.
    fn fine(&self, env: &Env, settings: &Sanctions, case: &Case, credits: u64) -> Option<Applied> {
        if !settings.fines {
            return None;
        }
        let sanction = format!("fine:{credits}");
        let week_start = case.at_ms.saturating_sub(WEEK_MS);
        let fined_this_week: u64 = self
            .lock()
            .applied
            .iter()
            .filter(|a| a.cid == case.cid && a.at_ms >= week_start)
            .map(|a| a.fine)
            .sum();
        let room = WEEKLY_FINE_CAP.saturating_sub(fined_this_week);
        let amount = credits.min(settings.max_fine).min(room);
        if amount == 0 {
            let why = if room == 0 {
                "weekly fine cap reached"
            } else {
                "max_fine is 0"
            };
            return Some(self.new_record(case, &sanction, &format!("not collected: {why}")));
        }
        let memo = format!(
            "sanction {} for {} (offence {})",
            case.law, case.tool, case.count
        );
        Some(match env.treasury.fine(case.cid, amount as i64, &memo) {
            Ok(entry) => {
                let mut record = self.new_record(case, &sanction, "applied");
                record.fine = amount;
                record.entry = Some(entry);
                record
            }
            Err(e) => self.new_record(case, &sanction, &format!("not collected: {e}")),
        })
    }

    /// Refuses the offending tool for `ms`. The suspension takes the id of its record.
    fn suspend(&self, settings: &Sanctions, case: &Case, ms: u64, label: &str) -> Option<Applied> {
        if !settings.suspensions {
            return None;
        }
        let until_ms = case.at_ms.saturating_add(ms);
        let mut record = self.new_record(case, &format!("{SUSPENSION}{label}"), "applied");
        record.until_ms = Some(until_ms);
        self.lock().suspensions.push(Suspension {
            id: record.id,
            cid: case.cid,
            tool: case.tool.to_string(),
            law: case.law.to_string(),
            until_ms,
            lifted: false,
        });
        Some(record)
    }

    /// A step the owner decides. Its record names the approval that asks, when one is queued.
    fn escalate(
        &self,
        env: &Env,
        case: &Case,
        name: &str,
        suspended_until: Option<u64>,
    ) -> Applied {
        let mut record = self.new_record(case, name, "escalated");
        let for_owner = matches!(name, "owner-review" | "retirement" | "demotion")
            || name.starts_with("lockout");
        if let (true, Some(ask)) = (for_owner, env.approvals) {
            let until = if name.starts_with("lockout") {
                suspended_until
            } else {
                None
            };
            let detail = json!({
                "cid": case.cid,
                "sanction_id": record.id,
                "law": case.law,
                "sanction": name,
                "until_ms": until,
            });
            let subject = format!("{} {name} for agent {}", case.law, case.cid);
            record.approval = Some(ask("sanction", &subject, detail));
        }
        record
    }

    /// The refusal for a call to `tool` while a suspension of it is in force, `None` when it may run.
    /// A suspension stops applying when sanctions or suspensions are switched off.
    pub fn suspended(
        &self,
        settings: &Sanctions,
        cid: u64,
        tool: &str,
        now_ms: u64,
    ) -> Option<String> {
        if !settings.enabled || !settings.suspensions {
            return None;
        }
        let state = self.lock();
        let found = state
            .suspensions
            .iter()
            .filter(|s| s.cid == cid && s.tool == tool && !s.lifted && s.until_ms > now_ms)
            .max_by_key(|s| s.until_ms)?;
        let minutes = (found.until_ms - now_ms).div_ceil(MINUTE_MS);
        Some(format!(
            "refused: {tool} is suspended for this agent for about {minutes} more minute(s) by a sanction for law {}",
            found.law
        ))
    }

    /// The records and suspensions for a query, newest first, at most `limit` of each.
    pub fn list(
        &self,
        cid: Option<u64>,
        law: Option<&str>,
        limit: usize,
        now_ms: u64,
    ) -> (Vec<Applied>, Vec<SuspensionView>) {
        let state = self.lock();
        let sanctions = state
            .applied
            .iter()
            .rev()
            .filter(|a| cid.is_none_or(|c| a.cid == c) && law.is_none_or(|l| a.law == l))
            .take(limit)
            .cloned()
            .collect();
        let suspensions = state
            .suspensions
            .iter()
            .rev()
            .filter(|s| cid.is_none_or(|c| s.cid == c) && law.is_none_or(|l| s.law == l))
            .take(limit)
            .map(|s| view(s, now_ms))
            .collect();
        (sanctions, suspensions)
    }

    /// The suspensions in force for a query, newest first. No limit: there are few of them, and a
    /// client that needs the suspended agents must not miss an old one behind newer records.
    pub fn active(&self, cid: Option<u64>, law: Option<&str>, now_ms: u64) -> Vec<SuspensionView> {
        let state = self.lock();
        state
            .suspensions
            .iter()
            .rev()
            .filter(|s| !s.lifted && s.until_ms > now_ms)
            .filter(|s| cid.is_none_or(|c| s.cid == c) && law.is_none_or(|l| s.law == l))
            .map(|s| view(s, now_ms))
            .collect()
    }

    /// Lifts the tool suspension recorded as `id`, while it is in force.
    pub fn lift(&self, id: u64, why: &str, now_ms: u64) -> Result<Applied, LiftError> {
        let mut state = self.lock();
        let at = state
            .applied
            .iter()
            .position(|a| a.id == id)
            .ok_or(LiftError::NotFound)?;
        if !state.applied[at].sanction.starts_with(SUSPENSION) {
            return Err(LiftError::NotSuspension);
        }
        let held = state
            .suspensions
            .iter()
            .position(|s| s.id == id && !s.lifted && s.until_ms > now_ms)
            .ok_or(LiftError::NotActive)?;
        state.suspensions[held].lifted = true;
        state.applied[at].lifted = Some(Lifted {
            at_ms: now_ms,
            by: "owner".to_string(),
            why: why.to_string(),
        });
        let record = state.applied[at].clone();
        self.save(&state);
        Ok(record)
    }

    /// Records the owner's decision `action` on the record `id`.
    pub fn decide_record(&self, id: u64, action: &str, now_ms: u64) -> Result<Applied, LiftError> {
        let mut state = self.lock();
        let at = state
            .applied
            .iter()
            .position(|a| a.id == id)
            .ok_or(LiftError::NotFound)?;
        state.applied[at].owner_action = Some(OwnerAction {
            action: action.to_string(),
            at_ms: now_ms,
        });
        let record = state.applied[at].clone();
        self.save(&state);
        Ok(record)
    }

    /// Whether the record `id` is a tool suspension.
    pub fn is_suspension(&self, id: u64) -> bool {
        self.lock()
            .applied
            .iter()
            .any(|a| a.id == id && a.sanction.starts_with(SUSPENSION))
    }

    /// The record as JSON: the database's `sanctions` component.
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&*self.lock()).unwrap_or_default()
    }

    /// Replaces the record with a snapshot.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let mut state: State = serde_json::from_str(json).map_err(|e| e.to_string())?;
        state.number();
        *self.lock() = state;
        Ok(())
    }

    /// The migration: reads `sanctions.json` into the record, when there is one to read.
    fn migrate_file(&self) {
        let Some(path) = &self.path else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        match serde_json::from_str::<State>(&text) {
            Ok(mut state) => {
                state.number();
                eprintln!(
                    "sanctions: migrated {} record(s) from {}",
                    state.applied.len(),
                    path.display()
                );
                *self.lock() = state;
            }
            Err(e) => eprintln!(
                "sanctions: {} is not readable, starting empty: {e}",
                path.display()
            ),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save(&self, state: &State) {
        let Some(path) = &self.path else { return };
        let saved = serde_json::to_vec_pretty(state)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, &bytes)
                    .and_then(|()| std::fs::rename(&tmp, path))
                    .map_err(|e| e.to_string())
            });
        if let Err(e) = saved {
            eprintln!("sanctions: could not save: {e}");
        }
    }
}

/// The warning's note in the agent's memory (the daily journal).
fn note(env: &Env, case: &Case) {
    let Some(memory) = env.memory else { return };
    let line = format!(
        "Warning from the plaza: law {} ({}) for {}; offence {} in 30 days.",
        case.law, case.title, case.tool, case.count
    );
    if let Err(e) = memory.journal(case.cid, &line, case.at_ms) {
        eprintln!("sanctions: could not note the warning: {e}");
    }
}

fn ledger(app: &App) -> &'static Ledger {
    static LEDGER: OnceLock<Ledger> = OnceLock::new();
    LEDGER.get_or_init(|| Ledger::new(Some(app.config.state_dir.join("sanctions.json"))))
}

/// The database's `sanctions` component, as JSON.
pub fn snapshot(app: &App) -> String {
    ledger(app).snapshot()
}

/// Restores the database's `sanctions` component, replacing the record.
pub fn restore(app: &App, json: &str) -> Result<(), String> {
    ledger(app).restore(json)
}

/// The one-time migration from `sanctions.json`. Run when the database holds no `sanctions` component,
/// or when there is no database. Nothing else reads the file.
pub fn migrate(app: &App) {
    static MIGRATED: Once = Once::new();
    MIGRATED.call_once(|| ledger(app).migrate_file());
}

/// The sanction hook: every violation the laws record comes here (`db::wire_violations`).
pub fn on_violation(app: &App, cid: u64, violation: &Violation, tool: &str) {
    let settings = app.settings.get().sanctions;
    if !settings.enabled {
        return;
    }
    let (title, severity) = match app
        .laws
        .catalog()
        .into_iter()
        .find(|law| law.id == violation.law)
    {
        Some(law) => (law.title, Severity::of(&law.severity)),
        None => (violation.law.clone(), Severity::Info),
    };
    let count = app
        .laws
        .recent(cid, violation.at_ms)
        .iter()
        .filter(|v| v["law"] == violation.law.as_str())
        .count();
    let ask = |kind: &str, subject: &str, detail: Value| {
        crate::approvals::submit(app, kind, subject, detail).id
    };
    let env = Env {
        treasury: &app.plaza.treasury,
        bus: &app.bus,
        memory: Some(&app.memory),
        approvals: Some(&ask),
    };
    let case = Case {
        cid,
        law: &violation.law,
        title: &title,
        severity,
        count,
        tool,
        at_ms: violation.at_ms,
    };
    ledger(app).apply(&env, &settings, &case);
}

/// The gate's check (`internal::tool_request`): the refusal while `tool` is suspended for `cid`.
pub fn suspended(app: &App, cid: u64, tool: &str) -> Option<String> {
    let settings = app.settings.get().sanctions;
    ledger(app).suspended(&settings, cid, tool, apz_plaza::now_ms())
}

/// Lifts a suspension and publishes its record as a `sanction` event.
fn lift_on(
    ledger: &Ledger,
    bus: &Bus,
    id: u64,
    why: &str,
    now_ms: u64,
) -> Result<Applied, LiftError> {
    let record = ledger.lift(id, why, now_ms)?;
    bus.publish("sanction", &record);
    Ok(record)
}

/// The owner's decision on the sanction `id`. Approve with no action or `acknowledge` acknowledges it.
/// Deny with `lift`, or with no action on a suspension, lifts it (refused unless it is active). The
/// action `retire` is recorded only: apid does not retire agents. Any other combination is refused.
fn settle(
    ledger: &Ledger,
    bus: &Bus,
    id: u64,
    approve: bool,
    action: Option<&str>,
    why: &str,
    now_ms: u64,
) -> Result<(), String> {
    let fail = |e: LiftError| e.message(id);
    match (approve, action) {
        (true, None | Some("acknowledge")) => {
            let record = ledger
                .decide_record(id, "acknowledge", now_ms)
                .map_err(fail)?;
            bus.publish("sanction", &record);
            Ok(())
        }
        (false, Some("lift")) => lift_on(ledger, bus, id, why, now_ms)
            .map(|_| ())
            .map_err(fail),
        (false, None) if ledger.is_suspension(id) => lift_on(ledger, bus, id, why, now_ms)
            .map(|_| ())
            .map_err(fail),
        (false, None) => Ok(()),
        (false, Some("retire")) => {
            let record = ledger.decide_record(id, "retire", now_ms).map_err(fail)?;
            eprintln!(
                "sanctions: the owner chose retire for agent {} (sanction {id}); apid does not retire agents, so nothing else changes",
                record.cid
            );
            bus.publish("sanction", &record);
            Ok(())
        }
        (_, Some(other)) => Err(format!(
            "`{other}` is not a decision for a sanction when approve is {approve}"
        )),
    }
}

/// The owner's decision on a `sanction` approval (`approvals::decide`). The item's detail names the record.
pub fn decided(
    app: &App,
    item: &Approval,
    approve: bool,
    action: Option<&str>,
) -> Result<(), String> {
    let Some(id) = item.detail.get("sanction_id").and_then(Value::as_u64) else {
        return Err("this sanction approval names no sanction".to_string());
    };
    let why = item.reason.as_deref().unwrap_or("");
    settle(
        ledger(app),
        &app.bus,
        id,
        approve,
        action,
        why,
        apz_plaza::now_ms(),
    )
}

/// The sanctions routes. Owner-only like the rest of `/v1/`: `hub_auth::gate` asks for the owner token
/// of any request that does not come from loopback.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/sanctions", list_route)
        .post("/v1/sanctions/{id}/lift", lift_route)
}

/// `GET /v1/sanctions?cid=&law=&limit=&active=1`: the sanctions and the suspensions, newest first.
/// `active=1` answers only the suspensions in force (no limit), with no sanctions.
fn list_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let cid = match req.query_param("cid").filter(|v| !v.is_empty()) {
        Some(value) => match value.parse::<u64>() {
            Ok(cid) => Some(cid),
            Err(_) => return Response::error(400, "cid must be a number"),
        },
        None => None,
    };
    let law = req.query_param("law").filter(|v| !v.is_empty());
    let now_ms = apz_plaza::now_ms();
    if matches!(req.query_param("active"), Some("1" | "true")) {
        let suspensions = ledger(app).active(cid, law, now_ms);
        return Response::json(200, &json!({ "sanctions": [], "suspensions": suspensions }));
    }
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .min(MAX_LIMIT);
    let (sanctions, suspensions) = ledger(app).list(cid, law, limit, now_ms);
    Response::json(
        200,
        &json!({ "sanctions": sanctions, "suspensions": suspensions }),
    )
}

/// `POST /v1/sanctions/{id}/lift` with `{"reason": str}`: the owner lifts an active suspension.
fn lift_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let Ok(id) = params.get("id").parse::<u64>() else {
        return Response::error(404, "no such sanction");
    };
    let body: Value = match req.json() {
        Ok(body) => body,
        Err(e) => {
            return Response::error(400, &format!("body must be {{\"reason\": string}}: {e}"));
        }
    };
    let why = body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    match lift_on(ledger(app), &app.bus, id, &why, apz_plaza::now_ms()) {
        Ok(record) => Response::json(200, &record),
        Err(LiftError::NotFound) => Response::error(404, &LiftError::NotFound.message(id)),
        Err(e) => Response::error(409, &e.message(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_plaza::events::EventBus;

    const CID: u64 = 7;
    const NOW: u64 = 1_800_000_000_000;

    /// An agent with `wallet` credits in its earnings, and the state the sanctions write to.
    struct World {
        treasury: Treasury,
        bus: Bus,
        memory: Memory,
        ledger: Ledger,
        settings: Sanctions,
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-sanctions-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn world(name: &str, wallet: i64) -> World {
        let treasury = Treasury::new(EventBus::new());
        if wallet > 0 {
            // The treasury's margin rule: pay at most a fifth of the task's real cost.
            treasury
                .fund_task("job", 6 * wallet + 1000)
                .expect("the task is funded");
            treasury
                .charge_spend(Some("job"), 5 * wallet, "real cost")
                .expect("the cost is spent");
            treasury
                .pay_agent("job", CID, wallet)
                .expect("the agent is paid");
        }
        World {
            treasury,
            bus: Bus::default(),
            memory: Memory::new(scratch(name)),
            ledger: Ledger::new(None),
            settings: Sanctions::default(),
        }
    }

    fn case(
        law: &'static str,
        severity: Severity,
        count: usize,
        tool: &'static str,
    ) -> Case<'static> {
        Case {
            cid: CID,
            law,
            title: "a law",
            severity,
            count,
            tool,
            at_ms: NOW,
        }
    }

    /// The id of the first tool-suspension record in `applied`.
    fn suspension_id(applied: &[Applied]) -> u64 {
        applied
            .iter()
            .find(|a| a.sanction.starts_with(SUSPENSION))
            .expect("a suspension")
            .id
    }

    impl World {
        fn env(&self) -> Env<'_> {
            Env {
                treasury: &self.treasury,
                bus: &self.bus,
                memory: Some(&self.memory),
                approvals: None,
            }
        }

        fn apply(&self, case: &Case) -> Vec<Applied> {
            self.ledger.apply(&self.env(), &self.settings, case)
        }

        fn sanctions(&self, case: &Case) -> Vec<String> {
            self.apply(case).into_iter().map(|a| a.sanction).collect()
        }

        fn wallet(&self) -> i128 {
            self.treasury.wallet(CID)
        }

        fn fines_in_journal(&self) -> Vec<String> {
            self.treasury
                .journal(100)
                .entries
                .into_iter()
                .filter(|e| e.kind == "fine")
                .map(|e| e.memo)
                .collect()
        }
    }

    #[test]
    fn the_schedule_is_the_yaml_table() {
        assert_eq!(steps(Severity::Medium, 1), [Step::Warning, Step::Fine(100)]);
        assert_eq!(steps(Severity::Info, 3), [Step::Fine(50)]);
        assert_eq!(steps(Severity::Info, 7), steps(Severity::Info, 3));
        assert_eq!(
            steps(Severity::High, 2),
            [
                Step::Suspend {
                    ms: 7 * DAY_MS,
                    label: "7d".into()
                },
                Step::Escalate("lockout:7d".into()),
                Step::Escalate("demotion".into())
            ]
        );
        assert_eq!(
            steps(Severity::Critical, 1),
            [
                Step::Suspend {
                    ms: 14 * DAY_MS,
                    label: "14d".into()
                },
                Step::Escalate("lockout:14d".into()),
                Step::Escalate("owner-review".into())
            ]
        );
        assert_eq!(Severity::of("critical"), Severity::Critical);
        assert_eq!(Severity::of("high"), Severity::High);
        assert_eq!(Severity::of("major"), Severity::Medium);
        assert_eq!(Severity::of("minor"), Severity::Low);
        assert_eq!(Severity::of("info"), Severity::Info);
        assert_eq!(Severity::of(""), Severity::Info);
    }

    #[test]
    fn minor_violations_warn_and_repeats_escalate() {
        // A minor violation is a warning only: no fine, no suspension, and the warning is noted.
        {
            let w = world("minor", 300);
            assert_eq!(
                w.sanctions(&case("T-001", Severity::Info, 1, "bash")),
                ["warning"]
            );
            assert_eq!(w.wallet(), 300);
            assert!(w.fines_in_journal().is_empty());
            assert!(w.ledger.suspended(&w.settings, CID, "bash", NOW).is_none());
            assert_eq!(
                w.memory.list(CID).len(),
                1,
                "the warning is noted in memory"
            );
        }
        // Repeated violations escalate with the offence count: warnings, then a fine.
        {
            let w = world("repeat", 300);
            assert_eq!(
                w.sanctions(&case("T-001", Severity::Info, 1, "bash")),
                ["warning"]
            );
            assert_eq!(
                w.sanctions(&case("T-001", Severity::Info, 2, "bash")),
                ["warning"]
            );
            assert_eq!(
                w.sanctions(&case("T-001", Severity::Info, 3, "bash")),
                ["fine:50"]
            );
            assert_eq!(w.wallet(), 250);
        }
    }

    #[test]
    fn a_fineable_violation_debits_the_wallet_by_the_schedule() {
        let w = world("fine", 300);
        let applied = w.apply(&case("T-004", Severity::Medium, 1, "bash"));
        assert_eq!(
            applied
                .iter()
                .map(|a| a.sanction.as_str())
                .collect::<Vec<_>>(),
            ["warning", "fine:100"]
        );
        assert_eq!(applied[1].fine, 100);
        assert!(applied[1].entry.is_some());
        assert_eq!(w.wallet(), 200);
        let fines = w.fines_in_journal();
        assert_eq!(fines.len(), 1);
        assert!(
            fines[0].contains("T-004"),
            "the fine's memo names the law: {fines:?}"
        );
    }

    #[test]
    fn a_fine_the_wallet_cannot_pay_is_recorded_as_not_collected() {
        let w = world("broke", 0);
        let applied = w.apply(&case("T-004", Severity::Medium, 1, "bash"));
        assert_eq!(applied[1].sanction, "fine:100");
        assert_eq!(applied[1].fine, 0);
        assert!(
            applied[1].outcome.starts_with("not collected"),
            "{}",
            applied[1].outcome
        );
        assert!(w.fines_in_journal().is_empty());
    }

    #[test]
    fn a_suspended_tool_is_refused_until_the_suspension_ends() {
        let w = world("suspend", 300);
        let sanctions = w.sanctions(&case("T-010", Severity::Low, 3, "bash"));
        assert_eq!(
            sanctions,
            ["fine:200", "tool-suspension:3d", "probation:3d"]
        );
        assert_eq!(w.wallet(), 100);
        let settings = &w.settings;
        assert!(
            w.ledger
                .suspended(settings, CID, "bash", NOW + 1)
                .is_some_and(|why| why.contains("T-010"))
        );
        assert!(
            w.ledger
                .suspended(settings, CID, "read_file", NOW + 1)
                .is_none(),
            "only the offending tool"
        );
        assert!(
            w.ledger.suspended(settings, 8, "bash", NOW + 1).is_none(),
            "only the offending agent"
        );
        assert!(
            w.ledger
                .suspended(settings, CID, "bash", NOW + 3 * DAY_MS - 1)
                .is_some()
        );
        assert!(
            w.ledger
                .suspended(settings, CID, "bash", NOW + 3 * DAY_MS)
                .is_none(),
            "it ends at its time"
        );
        // The list shows the same suspension as active until the same moment.
        let (_, listed) = w.ledger.list(None, None, 100, NOW + 1);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].active);
        let (_, later) = w.ledger.list(None, None, 100, NOW + 3 * DAY_MS);
        assert!(!later[0].active, "active flips at until_ms");
    }

    #[test]
    fn critical_violations_suspend_and_escalate_to_the_owner() {
        let w = world("critical", 300);
        assert_eq!(
            w.sanctions(&case("C-001a", Severity::Critical, 1, "bash")),
            ["tool-suspension:14d", "lockout:14d", "owner-review"]
        );
        assert_eq!(w.wallet(), 300, "no fine on a critical first offence");
    }

    #[test]
    fn sanction_switches_stop_what_they_name() {
        // Disabled sanctions apply nothing, and a running suspension stops with them.
        {
            let mut w = world("disabled", 300);
            w.apply(&case("C-001a", Severity::Critical, 1, "bash"));
            w.settings.enabled = false;
            assert!(
                w.apply(&case("T-004", Severity::Medium, 1, "read_file"))
                    .is_empty()
            );
            assert_eq!(w.wallet(), 300);
            assert!(w.fines_in_journal().is_empty());
            assert!(
                w.ledger
                    .suspended(&w.settings, CID, "bash", NOW + 1)
                    .is_none(),
                "a suspension stops with sanctions off"
            );
        }
        // Fines and suspensions can be switched off separately: only probation applies.
        {
            let mut w = world("switches", 300);
            w.settings.fines = false;
            w.settings.suspensions = false;
            assert_eq!(
                w.sanctions(&case("T-010", Severity::Low, 3, "bash")),
                ["probation:3d"]
            );
            assert_eq!(w.wallet(), 300);
            assert!(
                w.ledger
                    .suspended(&w.settings, CID, "bash", NOW + 1)
                    .is_none()
            );
        }
    }

    #[test]
    fn max_fine_and_the_weekly_cap_bound_the_fines() {
        let mut w = world("caps", 1500);
        w.settings.max_fine = 100;
        let applied = w.apply(&case("T-004", Severity::High, 1, "bash"));
        assert_eq!(applied[0].fine, 100, "max_fine caps the schedule's 300");
        w.settings.max_fine = 300;
        for offset in 1..4 {
            let mut again = case("T-004", Severity::High, 1, "bash");
            again.at_ms = NOW + offset;
            w.apply(&again);
        }
        // 100 (capped above) + 300 + 300 + 300 = 1000; the next fine gets only what the week has left.
        let mut last = case("T-004", Severity::High, 1, "bash");
        last.at_ms = NOW + 10;
        let fine = w
            .apply(&last)
            .iter()
            .find(|a| a.sanction.starts_with("fine"))
            .map(|a| a.fine);
        assert_eq!(fine, Some(0), "the week's 1000 is spent");
    }

    #[test]
    fn sanctions_are_published_on_the_bus() {
        let w = world("bus", 300);
        let rx = w.bus.subscribe();
        w.apply(&case("T-001", Severity::Info, 1, "bash"));
        let frame = rx.try_recv().expect("a sanction event");
        assert_eq!(frame.kind, "sanction");
        assert!(
            frame.data.contains("\"sanction\":\"warning\""),
            "{}",
            frame.data
        );
    }

    #[test]
    fn a_suspension_survives_a_restart() {
        let path = scratch("restart").join("sanctions.json");
        let w = world("restart-env", 300);
        Ledger::new(Some(path.clone())).apply(
            &w.env(),
            &w.settings,
            &case("T-010", Severity::Low, 3, "bash"),
        );
        let reloaded = Ledger::new(Some(path));
        reloaded.migrate_file();
        assert!(
            reloaded
                .suspended(&w.settings, CID, "bash", NOW + 1)
                .is_some()
        );
    }

    #[test]
    fn the_fine_cap_is_checked_when_settings_are_saved() {
        let store = crate::settings::Store::load(&scratch("settings-cap"), false)
            .expect("the settings load");
        let mut over = store.get();
        over.sanctions.max_fine = FINES_PER_WEEK_MAX + 1;
        assert!(store.put(over).is_err());
        let mut at_cap = store.get();
        at_cap.sanctions.max_fine = FINES_PER_WEEK_MAX;
        assert_eq!(
            store
                .put(at_cap)
                .expect("the cap is allowed")
                .sanctions
                .max_fine,
            FINES_PER_WEEK_MAX
        );
    }

    #[test]
    fn old_settings_load_with_the_sanctions_defaults() {
        let old: crate::settings::Settings = serde_json::from_str("{\"max_turns\": 24}").unwrap();
        assert_eq!(old.sanctions, Sanctions::default());
        assert!(
            Sanctions::default().enabled
                && Sanctions::default().fines
                && Sanctions::default().suspensions
        );
    }

    #[test]
    fn the_list_filters_by_agent_and_law_newest_first() {
        let w = world("list", 0);
        w.apply(&case("T-001", Severity::Info, 1, "bash"));
        w.apply(&Case {
            cid: 8,
            ..case("T-004", Severity::Info, 1, "bash")
        });
        w.apply(&case("T-004", Severity::Info, 1, "read_file"));
        let (all, _) = w.ledger.list(None, None, 100, NOW);
        assert_eq!(all.len(), 3);
        assert!(
            all.windows(2).all(|pair| pair[0].id > pair[1].id),
            "newest first"
        );
        assert_eq!(w.ledger.list(Some(CID), None, 100, NOW).0.len(), 2);
        let (by_law, _) = w.ledger.list(None, Some("T-004"), 100, NOW);
        assert_eq!(by_law.iter().map(|a| a.cid).collect::<Vec<_>>(), [CID, 8]);
        assert!(w.ledger.list(Some(8), Some("T-001"), 100, NOW).0.is_empty());
        assert_eq!(w.ledger.list(None, None, 1, NOW).0.len(), 1, "the limit");
    }

    #[test]
    fn the_active_list_keeps_an_old_suspension_past_the_limit() {
        // Oldest first in the file: one old suspension in force, 120 more in force, 150 lifted, one run out.
        let mut suspensions =
            vec![json!({ "cid": 1, "tool": "bash", "law": "T-010", "until_ms": NOW + DAY_MS })];
        suspensions.extend((0..120).map(
            |_| json!({ "cid": 4, "tool": "bash", "law": "T-010", "until_ms": NOW + DAY_MS }),
        ));
        suspensions.extend((0..150).map(|_| json!({ "cid": 2, "tool": "bash", "law": "T-010", "until_ms": NOW + DAY_MS, "lifted": true })));
        suspensions.push(json!({ "cid": 3, "tool": "bash", "law": "T-010", "until_ms": NOW - 1 }));
        let ledger = Ledger::new(None);
        ledger
            .restore(&json!({ "applied": [], "suspensions": suspensions }).to_string())
            .expect("the snapshot restores");
        let (_, newest) = ledger.list(None, None, DEFAULT_LIMIT, NOW);
        assert!(
            newest.iter().all(|s| s.cid != 1),
            "the newest 100 miss the old suspension"
        );
        let active = ledger.active(None, None, NOW);
        assert_eq!(
            active.len(),
            121,
            "every suspension in force, with no limit"
        );
        assert_eq!(
            active.last().map(|s| s.cid),
            Some(1),
            "newest first: the old one comes last"
        );
        assert!(active.iter().all(|s| s.active));
        assert_eq!(ledger.active(Some(1), Some("T-010"), NOW).len(), 1);
        assert!(
            ledger.active(Some(2), None, NOW).is_empty(),
            "lifted is not in force"
        );
        assert!(
            ledger.active(Some(3), None, NOW).is_empty(),
            "run out is not in force"
        );
        assert!(ledger.active(None, Some("T-999"), NOW).is_empty());
    }

    #[test]
    fn the_owner_lifts_a_suspension_once() {
        let w = world("lift", 300);
        let applied = w.apply(&case("T-010", Severity::Low, 3, "bash"));
        let id = suspension_id(&applied);
        let lifted =
            lift_on(&w.ledger, &w.bus, id, "appeal accepted", NOW + 1).expect("the first lift");
        let lift = lifted.lifted.expect("the record shows the lift");
        assert_eq!(
            (lift.by.as_str(), lift.why.as_str(), lift.at_ms),
            ("owner", "appeal accepted", NOW + 1)
        );
        assert!(
            w.ledger
                .suspended(&w.settings, CID, "bash", NOW + 2)
                .is_none(),
            "the tool runs again"
        );
        assert_eq!(
            lift_on(&w.ledger, &w.bus, id, "again", NOW + 2),
            Err(LiftError::NotActive),
            "a second lift is 409"
        );
        assert_eq!(
            lift_on(&w.ledger, &w.bus, 9_999, "", NOW + 2),
            Err(LiftError::NotFound),
            "an unknown id is 404"
        );
        let warning = applied[0].id;
        assert_eq!(
            lift_on(&w.ledger, &w.bus, warning, "", NOW + 2),
            Err(LiftError::NotSuspension)
        );
    }

    #[test]
    fn a_decided_lift_marks_the_suspension_lifted() {
        let w = world("decided", 300);
        let id = suspension_id(&w.apply(&case("T-010", Severity::Low, 3, "bash")));
        assert_eq!(
            settle(&w.ledger, &w.bus, id, false, Some("lift"), "", NOW + 1),
            Ok(())
        );
        assert!(
            w.ledger
                .suspended(&w.settings, CID, "bash", NOW + 2)
                .is_none()
        );
        assert!(
            settle(&w.ledger, &w.bus, id, false, Some("lift"), "", NOW + 2).is_err(),
            "not active any more"
        );
        assert!(
            settle(&w.ledger, &w.bus, id, true, Some("lift"), "", NOW + 2).is_err(),
            "approve cannot lift"
        );
        assert!(
            settle(
                &w.ledger,
                &w.bus,
                id,
                false,
                Some("acknowledge"),
                "",
                NOW + 2
            )
            .is_err()
        );
    }

    #[test]
    fn acknowledge_and_retire_are_recorded_on_the_sanction() {
        let w = world("acknowledge", 300);
        let applied = w.apply(&case("C-001a", Severity::Critical, 1, "bash"));
        let owner_review = applied
            .iter()
            .find(|a| a.sanction == "owner-review")
            .expect("the escalation")
            .id;
        assert_eq!(
            settle(&w.ledger, &w.bus, owner_review, true, None, "", NOW + 1),
            Ok(())
        );
        assert_eq!(
            settle(
                &w.ledger,
                &w.bus,
                owner_review,
                false,
                Some("retire"),
                "",
                NOW + 2
            ),
            Ok(())
        );
        let (records, _) = w.ledger.list(None, None, 100, NOW);
        let decided = records
            .iter()
            .find(|a| a.id == owner_review)
            .expect("the record");
        assert_eq!(
            decided.owner_action.as_ref().map(|o| o.action.as_str()),
            Some("retire"),
            "the last decision"
        );
        assert!(
            settle(
                &w.ledger,
                &w.bus,
                owner_review,
                true,
                Some("retire"),
                "",
                NOW
            )
            .is_err()
        );
        assert!(
            settle(&w.ledger, &w.bus, 9_999, true, None, "", NOW).is_err(),
            "an unknown sanction"
        );
    }

    #[test]
    fn an_escalation_asks_the_owner_through_an_approval() {
        let w = world("escalate", 300);
        let queue = crate::approvals::Store::load(&scratch("escalate-queue"));
        let ask =
            |kind: &str, subject: &str, detail: Value| queue.submit(CID, kind, subject, detail).id;
        let env = Env {
            treasury: &w.treasury,
            bus: &w.bus,
            memory: Some(&w.memory),
            approvals: Some(&ask),
        };
        let applied = w.ledger.apply(
            &env,
            &w.settings,
            &case("C-001a", Severity::Critical, 1, "bash"),
        );
        let owner = applied
            .iter()
            .find(|a| a.sanction == "owner-review")
            .expect("an owner-review step");
        let approval = owner.approval.clone().expect("the step names its approval");
        let pending = queue.list();
        let item = pending
            .iter()
            .find(|a| a.id == approval)
            .expect("the approval is queued");
        assert_eq!(item.kind, "sanction");
        assert_eq!(item.state, crate::approvals::State::Pending);
        assert_eq!(item.detail["sanction_id"].as_u64(), Some(owner.id));
        assert_eq!(item.subject, "C-001a owner-review for agent 7");
        assert_eq!(
            pending.len(),
            2,
            "the lockout and the owner-review ask; the suspension does not"
        );
        let suspension = applied
            .iter()
            .find(|a| a.sanction == "tool-suspension:14d")
            .expect("the suspension");
        assert!(suspension.approval.is_none());
    }

    #[test]
    fn the_snapshot_restores_the_record_and_its_counter() {
        let w = world("snapshot", 300);
        w.apply(&case("T-010", Severity::Low, 3, "bash"));
        let json = w.ledger.snapshot();
        let restored = Ledger::new(None);
        restored.restore(&json).expect("the snapshot restores");
        assert_eq!(
            restored.snapshot(),
            json,
            "the round trip keeps every field"
        );
        let next = restored.new_record(
            &case("T-001", Severity::Info, 1, "bash"),
            "warning",
            "applied",
        );
        assert_eq!(next.id, 4, "ids 1 to 3 are taken; the counter survives");
        assert!(
            restored
                .suspended(&w.settings, CID, "bash", NOW + 1)
                .is_some()
        );
        assert!(restored.restore("not json").is_err());
    }

    #[test]
    fn the_old_file_migrates_and_numbers_its_records() {
        let path = scratch("migrate").join("sanctions.json");
        // A file from before ids: no id, no counter, no lift.
        let old = r#"{"applied":[{"cid":7,"law":"T-001","title":"a law","severity":"info","count":1,"tool":"bash","at_ms":1800000000000,"sanction":"warning","outcome":"applied","fine":0,"entry":null,"until_ms":null},{"cid":7,"law":"T-010","title":"a law","severity":"low","count":3,"tool":"bash","at_ms":1800000000000,"sanction":"tool-suspension:3d","outcome":"applied","fine":0,"entry":null,"until_ms":1800259200000}],"suspensions":[{"cid":7,"tool":"bash","law":"T-010","until_ms":1800259200000}]}"#;
        std::fs::write(&path, old).unwrap();
        let ledger = Ledger::new(Some(path));
        ledger.migrate_file();
        let (sanctions, suspensions) = ledger.list(None, None, 100, NOW);
        assert_eq!(sanctions.iter().map(|a| a.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(suspensions[0].id, 2, "the suspension takes its record's id");
        assert!(suspensions[0].active);
        assert!(
            ledger
                .suspended(&Sanctions::default(), CID, "bash", NOW + 1)
                .is_some()
        );
        assert_eq!(
            lift_on(&ledger, &Bus::default(), 2, "migrated", NOW + 1).map(|a| a.id),
            Ok(2)
        );
    }
}
