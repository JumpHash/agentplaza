//! Budgets (PLAN 4.6, 10; W12, W13): per project and per task, in dollars or in tokens.
//!
//! - A project's budget: `cap`, `spent`, and `reserved` (what its running tasks may still spend). What it has left is
//!   `cap - spent - reserved - held`. A long-lived session (the planner) charges its project directly, under the
//!   project's `monthly_cap`.
//! - A task's budget: its own `cap` (in either unit), reserved from its project when the task starts (`start_task`);
//!   the unspent reservation goes back when it ends (`end_task`).
//! - Every model call is held before it is sent: its worst case (its input and its `max_tokens` of output, at its
//!   model's price) must fit in what the task has left ([`Budgets::hold`]); a call that does not fit is not sent, and
//!   the caller pauses the session and asks the owner with a `budget` card. After the call the actual cost is charged
//!   and the hold released ([`Budgets::settle`]).
//! - A task whose unit differs from its project's is checked in its own unit and charged to the project in the
//!   project's: every amount carries both micro-dollars and tokens.
//! - Writes go through to `plaza.db` (FULL): a hold is acknowledged by the writer before the call is sent, so a hold the
//!   hub answered survives a power loss; settles and the rest are queued. Every change is a ledger entry
//!   (`budget_entry`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::events::Bus;

/// A budget's unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// Micro-dollars.
    Usd,
    Tokens,
}

/// Whose budget it is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, apz_store::Column,
)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Project,
    Task,
}

/// An amount in both units (what a call costs or may cost).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    pub micros: u64,
    pub tokens: u64,
}

impl Amount {
    pub fn of(micros: u64, tokens: u64) -> Amount {
        Amount { micros, tokens }
    }

    /// The amount in `unit`.
    pub fn in_unit(&self, unit: Unit) -> u64 {
        match unit {
            Unit::Usd => self.micros,
            Unit::Tokens => self.tokens,
        }
    }
}

/// One budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "budget")]
pub struct Budget {
    #[table(key)]
    pub owner_kind: OwnerKind,
    #[table(key)]
    pub owner: String,
    /// For a task, its project; for a project, itself.
    pub project: String,
    pub unit: Unit,
    /// In the unit (micro-dollars or tokens); none means no cap.
    pub cap: Option<u64>,
    /// The share of the cap at which the owner is told (80 by default).
    pub warn_at_pct: u8,
    /// A project's cap per month for its long-lived sessions; none means no cap.
    pub monthly_cap: Option<u64>,
    /// A project's disk quota in bytes (`disk.rs`); none means no quota.
    pub disk_quota_bytes: Option<u64>,
    /// The month (`yyyymm`) `month_spent` counts.
    pub month: u32,
    pub month_spent: u64,
    /// A project: what its running tasks may still spend. A task: what it reserved from its project (project unit).
    pub reserved: u64,
    /// A task: how much of its reservation it has drawn (project unit).
    pub drawn: u64,
    pub spent: u64,
    /// The worst case of the calls in flight.
    pub held: u64,
    /// The owner paused it, or it reached its cap and waits for a raise.
    pub paused: bool,
    /// The warning at `warn_at_pct` was given.
    pub warned: bool,
    pub updated_ms: u64,
}

impl Budget {
    /// The budget as the consoles read it (`GET /v1/.../budget`, and the `budget` frame): the record, what is left,
    /// and the share spent.
    pub fn view(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["left"] = json!(self.left());
        v["spent_pct"] = json!(
            self.cap
                .filter(|c| *c > 0)
                .map(|c| (self.spent as f64 * 100.0 / c as f64).round())
        );
        v
    }

    /// Whether the budget has spend in its unit: a call charged or in flight, or money reserved or drawn. Its unit
    /// cannot change then.
    pub fn has_spend(&self) -> bool {
        self.spent > 0 || self.held > 0 || self.reserved > 0 || self.drawn > 0
    }

    /// What is left to spend now; none when uncapped.
    pub fn left(&self) -> Option<u64> {
        let used = self.spent
            + self.held
            + if self.owner_kind == OwnerKind::Project {
                self.reserved
            } else {
                0
            };
        self.cap.map(|cap| cap.saturating_sub(used))
    }

    fn warn_due(&self) -> bool {
        match self.cap {
            Some(cap) if cap > 0 && !self.warned => {
                self.spent.saturating_mul(100) >= cap.saturating_mul(u64::from(self.warn_at_pct))
            }
            _ => false,
        }
    }
}

/// What a ledger entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Set,
    Reserve,
    Hold,
    Charge,
    Release,
    Raise,
}

/// One ledger entry (table `budget_entry`, appended as it happens).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "budget_entry")]
pub struct Entry {
    pub owner_kind: OwnerKind,
    #[table(index)]
    pub owner: String,
    pub kind: EntryKind,
    /// In the owner's unit.
    pub amount: u64,
    pub micros: u64,
    pub tokens: u64,
    pub session: Option<String>,
    pub model: Option<String>,
    pub at_ms: u64,
}

/// A call's hold: given by [`Budgets::hold`], returned to [`Budgets::settle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    /// The task held on, or the project for a long-lived session.
    pub owner_kind: OwnerKind,
    pub owner: String,
    pub worst: Amount,
}

/// Why a call or a start was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Short {
    /// The task (or project) has `left` and the call needs `needed`, in its unit.
    Cap {
        owner_kind: OwnerKind,
        owner: String,
        unit: Unit,
        needed: u64,
        left: u64,
    },
    /// The project's monthly cap for long-lived sessions.
    Monthly {
        project: String,
        needed: u64,
        left: u64,
    },
    /// It is paused (by the owner, or at its cap waiting for a raise).
    Paused {
        owner_kind: OwnerKind,
        owner: String,
    },
}

/// What settling a call told the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A budget passed its warning share.
    Warn {
        owner_kind: OwnerKind,
        owner: String,
        spent: u64,
        cap: u64,
    },
}

/// Why a budget could not be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The budget has spend in its unit, so the unit cannot change (a 409 at the routes).
    UnitLocked { unit: Unit },
    /// The change could not be written to `plaza.db`.
    Store(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::UnitLocked { unit } => {
                let unit = match unit {
                    Unit::Usd => "usd",
                    Unit::Tokens => "tokens",
                };
                write!(
                    f,
                    "this budget has spend in {unit}; its unit can no longer change"
                )
            }
            Refused::Store(e) => f.write_str(e),
        }
    }
}

fn month_of(ms: u64) -> u32 {
    // Days since the epoch → civil date (Howard Hinnant's algorithm), enough for yyyymm.
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year * 100 + month) as u32
}

/// Where the budgets' writes go: `plaza.db`'s writer, or nowhere (tests).
pub type Sink = Option<Arc<apz_store::Db>>;

/// Every budget.
pub struct Budgets {
    budgets: Mutex<BTreeMap<(OwnerKind, String), Budget>>,
    sink: Mutex<Sink>,
    /// The bus each change is announced on, as a `budget` frame (none in tests that do not listen).
    bus: OnceLock<Arc<Bus>>,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            budgets: Mutex::new(BTreeMap::new()),
            sink: Mutex::new(None),
            bus: OnceLock::new(),
        }
    }
}

type Key = (OwnerKind, String);

impl Budgets {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<Key, Budget>> {
        self.budgets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Budgets that announce every change they make on `bus`.
    pub fn announcing(bus: Arc<Bus>) -> Budgets {
        let budgets = Budgets::default();
        let _ = budgets.bus.set(bus);
        budgets
    }

    /// The one place a budget's change reaches the consoles: a `budget` frame with the budget's view. Called after the
    /// change is made, outside the lock.
    fn announce<'a>(&self, changed: impl IntoIterator<Item = &'a Budget>) {
        if let Some(bus) = self.bus.get() {
            for b in changed {
                bus.publish("budget", &b.view());
            }
        }
    }

    /// Sends every later change to `db` (the hub's `plaza.db`).
    pub fn attach(&self, db: Arc<apz_store::Db>) {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(db);
    }

    fn sink(&self) -> Sink {
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Writes `budgets` and `entries`: waiting for the commit when `durable` (a hold), queued otherwise.
    fn persist(
        &self,
        budgets: Vec<Budget>,
        entries: Vec<Entry>,
        durable: bool,
    ) -> Result<(), String> {
        let Some(db) = self.sink() else {
            return Ok(());
        };
        let op =
            move |conn: &apz_store::rusqlite::Connection| -> Result<(), apz_store::StoreError> {
                let mut rows = apz_store::RowSet::list(def(), budgets.iter())?;
                // The budgets' rows are upserted one by one (a whole-table diff is not this table's way).
                for (table, list) in std::mem::take(&mut rows.tables) {
                    if table != def().name {
                        continue;
                    }
                    for row in list {
                        let mut changes = apz_store::table::Changes::default();
                        changes.upserts.push((table.clone(), row));
                        apz_store::table::apply(conn, def(), &changes)?;
                    }
                }
                // The ledger only appends: each entry's key (`pos`, the row id) is left to SQLite.
                let entries_def = entries_def();
                let mut appended = apz_store::table::Changes::default();
                for entry in &entries {
                    let mut set = apz_store::RowSet::new();
                    entry_put(entry, entries_def, &mut set)?;
                    for row in set.tables.remove(&entries_def.name).unwrap_or_default() {
                        appended.upserts.push((entries_def.name.clone(), row));
                    }
                }
                apz_store::table::apply(conn, entries_def, &appended)?;
                Ok(())
            };
        if durable {
            db.write(op).map(|_| ()).map_err(|e| e.to_string())
        } else {
            db.send(op).map_err(|e| e.to_string())
        }
    }

    pub fn get(&self, owner_kind: OwnerKind, owner: &str) -> Option<Budget> {
        self.lock().get(&(owner_kind, owner.to_string())).cloned()
    }

    /// Every budget of `project` (the project's and its tasks').
    pub fn of_project(&self, project: &str) -> Vec<Budget> {
        self.lock()
            .values()
            .filter(|b| b.project == project)
            .cloned()
            .collect()
    }

    /// Sets a project's budget (unit, cap, warning share, monthly cap, disk quota). A unit cannot change once the budget
    /// has spend in it ([`Refused::UnitLocked`]); with no spend, or the same unit, the rest is set as asked.
    pub fn set_project(
        &self,
        project: &str,
        unit: Unit,
        cap: Option<u64>,
        warn_at_pct: u8,
        monthly_cap: Option<u64>,
        disk_quota_bytes: Option<u64>,
        now_ms: u64,
    ) -> Result<Budget, Refused> {
        let budget = {
            let mut budgets = self.lock();
            let b = budgets
                .entry((OwnerKind::Project, project.to_string()))
                .or_insert_with(|| empty(OwnerKind::Project, project, project, unit, now_ms));
            if b.unit != unit && b.has_spend() {
                return Err(Refused::UnitLocked { unit: b.unit });
            }
            b.unit = unit;
            b.cap = cap;
            b.warn_at_pct = warn_at_pct.clamp(1, 100);
            b.monthly_cap = monthly_cap;
            b.disk_quota_bytes = disk_quota_bytes;
            b.warned = b.warn_due() && false;
            b.paused = b.left() == Some(0) && cap.is_some();
            b.updated_ms = now_ms;
            b.clone()
        };
        self.persist(
            vec![budget.clone()],
            vec![entry(
                &budget,
                EntryKind::Set,
                cap.unwrap_or(0),
                Amount::default(),
                None,
                None,
                now_ms,
            )],
            true,
        )
        .map_err(Refused::Store)?;
        self.announce([&budget]);
        Ok(budget)
    }

    /// Sets a task's own budget. Its unit cannot change once it has spend ([`Refused::UnitLocked`]).
    pub fn set_task(
        &self,
        task: &str,
        project: &str,
        unit: Unit,
        cap: Option<u64>,
        now_ms: u64,
    ) -> Result<Budget, Refused> {
        let budget = {
            let mut budgets = self.lock();
            let b = budgets
                .entry((OwnerKind::Task, task.to_string()))
                .or_insert_with(|| empty(OwnerKind::Task, task, project, unit, now_ms));
            if b.unit != unit && b.has_spend() {
                return Err(Refused::UnitLocked { unit: b.unit });
            }
            b.unit = unit;
            b.cap = cap;
            b.paused = b.left() == Some(0) && cap.is_some();
            b.updated_ms = now_ms;
            b.clone()
        };
        self.persist(
            vec![budget.clone()],
            vec![entry(
                &budget,
                EntryKind::Set,
                cap.unwrap_or(0),
                Amount::default(),
                None,
                None,
                now_ms,
            )],
            true,
        )
        .map_err(Refused::Store)?;
        self.announce([&budget]);
        Ok(budget)
    }

    /// Reserves `reserve` (in the project's unit: the task's cap when the units agree, else the planner's estimate) from
    /// the task's project, as the task starts. Refused when the project cannot reserve it.
    pub fn start_task(
        &self,
        task: &str,
        project: &str,
        reserve: u64,
        now_ms: u64,
    ) -> Result<(), Short> {
        let (task_budget, project_budget) = {
            let mut budgets = self.lock();
            // A project with no budget runs unmetered.
            let Some(p) = budgets.get_mut(&(OwnerKind::Project, project.to_string())) else {
                return Ok(());
            };
            if p.paused {
                return Err(Short::Paused {
                    owner_kind: OwnerKind::Project,
                    owner: p.owner.clone(),
                });
            }
            if let Some(left) = p.left()
                && left < reserve
            {
                return Err(Short::Cap {
                    owner_kind: OwnerKind::Project,
                    owner: p.owner.clone(),
                    unit: p.unit,
                    needed: reserve,
                    left,
                });
            }
            p.reserved += reserve;
            p.updated_ms = now_ms;
            let p = p.clone();
            // A task with no budget of its own gets one here, with no cap, in its project's unit: it reserves from the
            // project all the same.
            let t = budgets
                .entry((OwnerKind::Task, task.to_string()))
                .or_insert_with(|| empty(OwnerKind::Task, task, project, p.unit, now_ms));
            t.reserved = reserve;
            t.drawn = 0;
            t.updated_ms = now_ms;
            (t.clone(), p)
        };
        let reserved = entry(
            &project_budget,
            EntryKind::Reserve,
            reserve,
            Amount::default(),
            None,
            None,
            now_ms,
        );
        self.announce([&task_budget, &project_budget]);
        self.persist(vec![task_budget, project_budget], vec![reserved], false)
            .map_err(|_| Short::Paused {
                owner_kind: OwnerKind::Task,
                owner: task.to_string(),
            })
    }

    /// Gives back what `task` did not spend of its reservation, as it ends.
    pub fn end_task(&self, task: &str, now_ms: u64) {
        let changed = {
            let mut budgets = self.lock();
            let Some(t) = budgets.get_mut(&(OwnerKind::Task, task.to_string())) else {
                return;
            };
            let unspent = t.reserved.saturating_sub(t.drawn);
            t.reserved = 0;
            t.drawn = 0;
            t.updated_ms = now_ms;
            let t = t.clone();
            let Some(p) = budgets.get_mut(&(OwnerKind::Project, t.project.clone())) else {
                return;
            };
            p.reserved = p.reserved.saturating_sub(unspent);
            p.updated_ms = now_ms;
            Some((t, p.clone(), unspent))
        };
        if let Some((t, p, unspent)) = changed {
            let released = entry(
                &p,
                EntryKind::Release,
                unspent,
                Amount::default(),
                None,
                None,
                now_ms,
            );
            self.announce([&t, &p]);
            let _ = self.persist(vec![t, p], vec![released], false);
        }
    }

    /// Holds a call's worst case on `task` (or, with no task, on `project` for a long-lived session). Refused when it
    /// does not fit: the caller does not send the call, pauses the session and asks the owner.
    pub fn hold(
        &self,
        task: Option<&str>,
        project: &str,
        worst: Amount,
        session: Option<&str>,
        model: Option<&str>,
        now_ms: u64,
    ) -> Result<Hold, Short> {
        let (budget, amount) = {
            let mut budgets = self.lock();
            let key = match task {
                Some(task) if budgets.contains_key(&(OwnerKind::Task, task.to_string())) => {
                    (OwnerKind::Task, task.to_string())
                }
                _ => (OwnerKind::Project, project.to_string()),
            };
            let Some(b) = budgets.get_mut(&key) else {
                // No budget set: nothing to check (a plaza with no budgets runs unmetered).
                return Ok(Hold {
                    owner_kind: key.0,
                    owner: key.1,
                    worst,
                });
            };
            if b.paused {
                return Err(Short::Paused {
                    owner_kind: key.0,
                    owner: key.1,
                });
            }
            let amount = worst.in_unit(b.unit);
            if let Some(left) = b.left()
                && left < amount
            {
                return Err(Short::Cap {
                    owner_kind: key.0,
                    owner: key.1,
                    unit: b.unit,
                    needed: amount,
                    left,
                });
            }
            if key.0 == OwnerKind::Project {
                let month = month_of(now_ms);
                if b.month != month {
                    b.month = month;
                    b.month_spent = 0;
                }
                if let Some(monthly) = b.monthly_cap {
                    let left = monthly.saturating_sub(b.month_spent);
                    if left < amount {
                        return Err(Short::Monthly {
                            project: b.owner.clone(),
                            needed: amount,
                            left,
                        });
                    }
                }
            }
            b.held += amount;
            b.updated_ms = now_ms;
            (b.clone(), amount)
        };
        let hold = Hold {
            owner_kind: budget.owner_kind,
            owner: budget.owner.clone(),
            worst,
        };
        let held = entry(
            &budget,
            EntryKind::Hold,
            amount,
            worst,
            session,
            model,
            now_ms,
        );
        self.announce([&budget]);
        // Acknowledged by the writer before the call goes out: a hold the hub answered survives a power loss.
        if self.persist(vec![budget], vec![held], true).is_err() {
            self.release(&hold, now_ms);
            return Err(Short::Paused {
                owner_kind: hold.owner_kind,
                owner: hold.owner,
            });
        }
        Ok(hold)
    }

    /// Releases a call's hold without charging it (the call failed).
    pub fn release(&self, hold: &Hold, now_ms: u64) {
        let released = {
            let mut budgets = self.lock();
            budgets
                .get_mut(&(hold.owner_kind, hold.owner.clone()))
                .map(|b| {
                    b.held = b.held.saturating_sub(hold.worst.in_unit(b.unit));
                    b.updated_ms = now_ms;
                    b.clone()
                })
        };
        if let Some(b) = released {
            self.announce([&b]);
        }
    }

    /// Releases a call's hold and charges what it really cost: to its task (and the task's draw on its project's
    /// reservation) and to its project. Returns the warnings it passed.
    pub fn settle(
        &self,
        hold: &Hold,
        actual: Amount,
        session: Option<&str>,
        model: Option<&str>,
        now_ms: u64,
    ) -> Vec<Notice> {
        let mut notices = Vec::new();
        let mut changed = Vec::new();
        let mut entries = Vec::new();
        {
            let mut budgets = self.lock();
            let key = (hold.owner_kind, hold.owner.clone());
            let Some(b) = budgets.get_mut(&key) else {
                return notices;
            };
            b.held = b.held.saturating_sub(hold.worst.in_unit(b.unit));
            let amount = actual.in_unit(b.unit);
            b.spent += amount;
            if b.owner_kind == OwnerKind::Project {
                b.month_spent += amount;
            }
            if b.warn_due() {
                b.warned = true;
                notices.push(Notice::Warn {
                    owner_kind: b.owner_kind,
                    owner: b.owner.clone(),
                    spent: b.spent,
                    cap: b.cap.unwrap_or(0),
                });
            }
            if b.left() == Some(0) {
                b.paused = true;
            }
            b.updated_ms = now_ms;
            let project = b.project.clone();
            entries.push(entry(
                b,
                EntryKind::Charge,
                amount,
                actual,
                session,
                model,
                now_ms,
            ));
            changed.push(b.clone());
            if hold.owner_kind == OwnerKind::Task
                && let Some(p) = budgets.get_mut(&(OwnerKind::Project, project.clone()))
            {
                let amount = actual.in_unit(p.unit);
                p.spent += amount;
                p.updated_ms = now_ms;
                entries.push(entry(
                    p,
                    EntryKind::Charge,
                    amount,
                    actual,
                    session,
                    model,
                    now_ms,
                ));
                let p_unit = p.unit;
                let p_snapshot = p.clone();
                // The task draws on its reservation: the project's reserve shrinks by what the task spent of it.
                if let Some(t) = budgets.get_mut(&key) {
                    let room = t.reserved.saturating_sub(t.drawn);
                    let draw = actual.in_unit(p_unit).min(room);
                    t.drawn += draw;
                    changed.pop();
                    changed.push(t.clone());
                    if let Some(p) = budgets.get_mut(&(OwnerKind::Project, project)) {
                        p.reserved = p.reserved.saturating_sub(draw);
                        if p.warn_due() {
                            p.warned = true;
                            notices.push(Notice::Warn {
                                owner_kind: OwnerKind::Project,
                                owner: p.owner.clone(),
                                spent: p.spent,
                                cap: p.cap.unwrap_or(0),
                            });
                        }
                        if p.left() == Some(0) {
                            p.paused = true;
                        }
                        changed.push(p.clone());
                    } else {
                        changed.push(p_snapshot);
                    }
                }
            }
        }
        self.announce(&changed);
        let _ = self.persist(changed, entries, false);
        notices
    }

    /// Raises a budget's cap (by an amount, or to one) and resumes it. Returns the budget as raised.
    pub fn raise(
        &self,
        owner_kind: OwnerKind,
        owner: &str,
        by: Option<u64>,
        to: Option<u64>,
        now_ms: u64,
    ) -> Option<Budget> {
        let budget = {
            let mut budgets = self.lock();
            let b = budgets.get_mut(&(owner_kind, owner.to_string()))?;
            let cap = match (to, by) {
                (Some(to), _) => to,
                (None, Some(by)) => b.cap.unwrap_or(b.spent).saturating_add(by),
                (None, None) => return Some(b.clone()),
            };
            b.cap = Some(cap);
            b.paused = b.left() == Some(0);
            b.warned = b.warn_due() && b.warned;
            b.updated_ms = now_ms;
            b.clone()
        };
        let raised = entry(
            &budget,
            EntryKind::Raise,
            budget.cap.unwrap_or(0),
            Amount::default(),
            None,
            None,
            now_ms,
        );
        self.announce([&budget]);
        let _ = self.persist(vec![budget.clone()], vec![raised], true);
        Some(budget)
    }

    /// Pauses or resumes a budget by hand.
    pub fn set_paused(
        &self,
        owner_kind: OwnerKind,
        owner: &str,
        paused: bool,
        now_ms: u64,
    ) -> Option<Budget> {
        let budget = {
            let mut budgets = self.lock();
            let b = budgets.get_mut(&(owner_kind, owner.to_string()))?;
            b.paused = paused;
            b.updated_ms = now_ms;
            b.clone()
        };
        self.announce([&budget]);
        let _ = self.persist(vec![budget.clone()], Vec::new(), true);
        Some(budget)
    }

    /// The budgets' rows, for the hub's store (the saver keeps them too; holds are also written through at once).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let budgets = self.lock();
        apz_store::RowSet::list(def(), budgets.values())
    }

    /// Replaces the budgets with what the store holds (at start). Holds of calls that were in flight are dropped: those
    /// calls ended with the hub.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Budget> = apz_store::table::load_list(conn, def())?;
        let mut budgets = BTreeMap::new();
        for mut b in stored {
            b.held = 0;
            budgets.insert((b.owner_kind, b.owner.clone()), b);
        }
        *self.lock() = budgets;
        Ok(())
    }

    /// The ledger of `owner`, newest first, at most `limit` entries.
    pub fn ledger(
        conn: &apz_store::rusqlite::Connection,
        owner_kind: OwnerKind,
        owner: &str,
        limit: usize,
    ) -> Result<Vec<Entry>, apz_store::StoreError> {
        use apz_store::Column;
        let mut stmt = conn.prepare_cached(crate::store::sql::BUDGET_LEDGER)?;
        let kind = owner_kind.to_value()?;
        let rows = stmt.query_map(
            apz_store::rusqlite::params![kind, owner, i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                let v = |i: usize| r.get::<_, apz_store::table::Value>(i);
                Ok((
                    v(0)?,
                    v(1)?,
                    v(2)?,
                    v(3)?,
                    v(4)?,
                    v(5)?,
                    v(6)?,
                    v(7)?,
                    v(8)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (ok, ow, k, a, m, t, se, mo, at) = row?;
            out.push(Entry {
                owner_kind: OwnerKind::from_value(ok)?,
                owner: String::from_value(ow)?,
                kind: EntryKind::from_value(k)?,
                amount: u64::from_value(a)?,
                micros: u64::from_value(m)?,
                tokens: u64::from_value(t)?,
                session: Option::<String>::from_value(se)?,
                model: Option::<String>::from_value(mo)?,
                at_ms: u64::from_value(at)?,
            });
        }
        Ok(out)
    }
}

fn empty(owner_kind: OwnerKind, owner: &str, project: &str, unit: Unit, now_ms: u64) -> Budget {
    Budget {
        owner_kind,
        owner: owner.to_string(),
        project: project.to_string(),
        unit,
        cap: None,
        warn_at_pct: 80,
        monthly_cap: None,
        disk_quota_bytes: None,
        month: month_of(now_ms),
        month_spent: 0,
        reserved: 0,
        drawn: 0,
        spent: 0,
        held: 0,
        paused: false,
        warned: false,
        updated_ms: now_ms,
    }
}

fn entry(
    b: &Budget,
    kind: EntryKind,
    amount: u64,
    both: Amount,
    session: Option<&str>,
    model: Option<&str>,
    now_ms: u64,
) -> Entry {
    Entry {
        owner_kind: b.owner_kind,
        owner: b.owner.clone(),
        kind,
        amount,
        micros: both.micros,
        tokens: both.tokens,
        session: session.map(str::to_string),
        model: model.map(str::to_string),
        at_ms: now_ms,
    }
}

/// The budgets' table.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Budget)
}

/// The ledger's table.
pub fn entries_def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Entry)
}

/// Puts one ledger entry as a row (keyed by a fresh position: the ledger only appends).
fn entry_put(
    e: &Entry,
    def: &apz_store::TableDef,
    out: &mut apz_store::RowSet,
) -> Result<(), apz_store::StoreError> {
    use apz_store::Table;
    // The ledger's key is its position; an append takes the next one in the statement (see apply_inserts).
    e.put(def, vec![apz_store::table::Value::Null], out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(dollars: f64) -> u64 {
        (dollars * 1_000_000.0) as u64
    }

    fn budgets() -> Budgets {
        let b = Budgets::default();
        b.set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 1)
            .unwrap();
        b.set_task("t1", "pr1", Unit::Usd, Some(usd(4.0)), 1)
            .unwrap();
        b
    }

    #[test]
    fn a_plain_task_reserves_from_its_project_and_gives_back_what_it_did_not_spend() {
        let b = Budgets::default();
        b.set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 1)
            .unwrap();
        // No budget of its own: the task starts, and reserves from the project's budget.
        b.start_task("plain", "pr1", usd(4.0), 2).unwrap();
        assert_eq!(b.get(OwnerKind::Task, "plain").unwrap().reserved, usd(4.0));
        assert_eq!(
            b.get(OwnerKind::Project, "pr1").unwrap().left(),
            Some(usd(6.0))
        );
        // The project's cap refuses a second plain task that does not fit.
        assert!(matches!(
            b.start_task("second", "pr1", usd(7.0), 2),
            Err(Short::Cap {
                owner_kind: OwnerKind::Project,
                ..
            })
        ));
        b.end_task("plain", 3);
        assert_eq!(
            b.get(OwnerKind::Project, "pr1").unwrap().left(),
            Some(usd(10.0))
        );
    }

    #[test]
    fn a_task_reserves_from_its_project_and_gives_back_what_it_did_not_spend() {
        let b = budgets();
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        assert_eq!(
            b.get(OwnerKind::Project, "pr1").unwrap().left(),
            Some(usd(6.0))
        );
        let hold = b
            .hold(
                Some("t1"),
                "pr1",
                Amount::of(usd(1.0), 1000),
                Some("s1"),
                Some("m"),
                3,
            )
            .unwrap();
        b.settle(&hold, Amount::of(usd(0.5), 400), Some("s1"), Some("m"), 4);
        let p = b.get(OwnerKind::Project, "pr1").unwrap();
        assert_eq!((p.spent, p.reserved), (usd(0.5), usd(3.5)));
        b.end_task("t1", 5);
        let p = b.get(OwnerKind::Project, "pr1").unwrap();
        assert_eq!(
            (p.spent, p.reserved, p.left()),
            (usd(0.5), 0, Some(usd(9.5)))
        );
    }

    #[test]
    fn a_project_that_cannot_reserve_refuses_the_task() {
        let b = budgets();
        assert!(matches!(
            b.start_task("t1", "pr1", usd(11.0), 2),
            Err(Short::Cap {
                owner_kind: OwnerKind::Project,
                ..
            })
        ));
    }

    #[test]
    fn a_call_that_does_not_fit_is_refused_and_the_cap_pauses_the_task() {
        let b = budgets();
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        let worst = Amount::of(usd(3.0), 0);
        let hold = b.hold(Some("t1"), "pr1", worst, None, None, 3).unwrap();
        assert!(
            matches!(b.hold(Some("t1"), "pr1", worst, None, None, 3), Err(Short::Cap { needed, left, .. }) if needed == usd(3.0) && left == usd(1.0)),
            "a second call's worst case does not fit beside the first's hold"
        );
        b.settle(&hold, Amount::of(usd(4.0), 0), None, None, 4);
        let t = b.get(OwnerKind::Task, "t1").unwrap();
        assert!(t.paused, "at its cap the task pauses");
        assert!(matches!(
            b.hold(Some("t1"), "pr1", Amount::of(1, 0), None, None, 5),
            Err(Short::Paused { .. })
        ));
        b.raise(OwnerKind::Task, "t1", Some(usd(2.0)), None, 6)
            .unwrap();
        assert!(
            b.hold(Some("t1"), "pr1", Amount::of(usd(1.0), 0), None, None, 7)
                .is_ok(),
            "a raise resumes it"
        );
    }

    #[test]
    fn the_warning_comes_once_at_its_share() {
        let b = budgets();
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        let hold = b
            .hold(Some("t1"), "pr1", Amount::of(usd(3.3), 0), None, None, 3)
            .unwrap();
        let notices = b.settle(&hold, Amount::of(usd(3.3), 0), None, None, 4);
        assert!(
            notices.iter().any(|n| matches!(
                n,
                Notice::Warn {
                    owner_kind: OwnerKind::Task,
                    ..
                }
            )),
            "{notices:?}"
        );
        let hold = b
            .hold(Some("t1"), "pr1", Amount::of(usd(0.1), 0), None, None, 5)
            .unwrap();
        assert!(
            b.settle(&hold, Amount::of(usd(0.1), 0), None, None, 6)
                .is_empty(),
            "not twice"
        );
    }

    #[test]
    fn a_token_task_is_checked_in_tokens_and_charged_to_its_dollar_project_in_dollars() {
        let b = Budgets::default();
        b.set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 1)
            .unwrap();
        b.set_task("t1", "pr1", Unit::Tokens, Some(2_000_000), 1)
            .unwrap();
        b.start_task("t1", "pr1", usd(2.0), 2).unwrap();
        assert!(matches!(
            b.hold(
                Some("t1"),
                "pr1",
                Amount::of(usd(0.01), 3_000_000),
                None,
                None,
                3
            ),
            Err(Short::Cap {
                unit: Unit::Tokens,
                ..
            })
        ));
        let hold = b
            .hold(
                Some("t1"),
                "pr1",
                Amount::of(usd(0.5), 500_000),
                None,
                None,
                4,
            )
            .unwrap();
        b.settle(&hold, Amount::of(usd(0.4), 400_000), None, None, 5);
        assert_eq!(b.get(OwnerKind::Task, "t1").unwrap().spent, 400_000);
        assert_eq!(b.get(OwnerKind::Project, "pr1").unwrap().spent, usd(0.4));
    }

    #[test]
    fn a_long_lived_session_charges_its_project_under_the_monthly_cap() {
        let b = Budgets::default();
        b.set_project("pr1", Unit::Usd, None, 80, Some(usd(1.0)), None, 1)
            .unwrap();
        let hold = b
            .hold(None, "pr1", Amount::of(usd(0.8), 0), None, None, 2)
            .unwrap();
        b.settle(&hold, Amount::of(usd(0.8), 0), None, None, 3);
        assert!(matches!(
            b.hold(None, "pr1", Amount::of(usd(0.5), 0), None, None, 4),
            Err(Short::Monthly { .. })
        ));
        // A new month starts the count again.
        let next_month = 40 * 86_400_000;
        assert!(
            b.hold(None, "pr1", Amount::of(usd(0.5), 0), None, None, next_month)
                .is_ok()
        );
    }

    #[test]
    fn a_disk_quota_is_set_beside_the_spending_and_can_be_cleared() {
        let b = budgets();
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        let hold = b
            .hold(Some("t1"), "pr1", Amount::of(usd(1.0), 0), None, None, 3)
            .unwrap();
        b.settle(&hold, Amount::of(usd(1.0), 0), None, None, 4);
        let set = b
            .set_project(
                "pr1",
                Unit::Usd,
                Some(usd(10.0)),
                80,
                None,
                Some(5 << 30),
                5,
            )
            .unwrap();
        assert_eq!(set.disk_quota_bytes, Some(5 << 30));
        assert_eq!(set.spent, usd(1.0), "a quota does not touch the spending");
        let cleared = b
            .set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 6)
            .unwrap();
        assert_eq!((cleared.disk_quota_bytes, cleared.spent), (None, usd(1.0)));
    }

    #[test]
    fn months_are_civil_months() {
        assert_eq!(month_of(0), 197_001);
        assert_eq!(month_of(1_792_000_000_000), 202_610); // 2026-10-14
    }

    #[test]
    fn holds_and_charges_reach_the_store_and_budgets_load_back() {
        let dir = tempfile::tempdir().unwrap();
        let ddl = format!("{}{}", def().ddl(), entries_def().ddl());
        let migrations: &'static [apz_store::Migration] =
            Box::leak(Box::new([apz_store::Migration {
                version: 1,
                name: "budgets",
                sql: Box::leak(ddl.into_boxed_str()),
            }]));
        let files: &'static [apz_store::FileSpec] = Box::leak(Box::new([apz_store::FileSpec {
            name: "plaza",
            file: "plaza.db",
            durability: apz_store::Durability::Scratch,
            migrations,
        }]));
        let store = apz_store::Store::open(dir.path(), files).unwrap();
        let db = Arc::clone(store.db("plaza"));
        let b = budgets();
        b.attach(Arc::clone(&db));
        b.set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 1)
            .unwrap();
        b.set_task("t1", "pr1", Unit::Usd, Some(usd(4.0)), 1)
            .unwrap();
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        let hold = b
            .hold(
                Some("t1"),
                "pr1",
                Amount::of(usd(1.0), 10),
                Some("s1"),
                Some("m"),
                3,
            )
            .unwrap();
        b.settle(&hold, Amount::of(usd(0.25), 5), Some("s1"), Some("m"), 4);
        db.flush().unwrap();
        let loaded = Budgets::default();
        db.read(|conn| loaded.load(conn)).unwrap();
        assert_eq!(
            loaded.get(OwnerKind::Task, "t1"),
            b.get(OwnerKind::Task, "t1")
        );
        let ledger = db
            .read(|conn| Budgets::ledger(conn, OwnerKind::Task, "t1", 10))
            .unwrap();
        assert_eq!(ledger.first().map(|e| e.kind), Some(EntryKind::Charge));
        assert!(ledger.iter().any(|e| e.kind == EntryKind::Hold));
    }

    /// The `budget` frames published since the subscription, as (owner, the frame's data).
    fn budget_frames(
        rx: &std::sync::mpsc::Receiver<Arc<crate::events::Frame>>,
    ) -> Vec<(String, Value)> {
        rx.try_iter()
            .filter(|f| f.kind == "budget")
            .map(|f| {
                let data: Value = serde_json::from_str(&f.data).unwrap();
                (data["owner"].as_str().unwrap().to_string(), data)
            })
            .collect()
    }

    #[test]
    fn every_change_of_a_budget_publishes_its_view_as_a_budget_frame() {
        let bus = Arc::new(Bus::default());
        let rx = bus.subscribe();
        let b = Budgets::announcing(Arc::clone(&bus));
        b.set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 1)
            .unwrap();
        b.set_task("t1", "pr1", Unit::Usd, Some(usd(4.0)), 1)
            .unwrap();
        let frames = budget_frames(&rx);
        assert_eq!(
            frames
                .iter()
                .map(|(owner, _)| owner.as_str())
                .collect::<Vec<_>>(),
            ["pr1", "t1"],
            "a set publishes its budget"
        );
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        let hold = b
            .hold(Some("t1"), "pr1", Amount::of(usd(1.0), 0), None, None, 3)
            .unwrap();
        b.settle(&hold, Amount::of(usd(0.5), 0), None, None, 4);
        let frames = budget_frames(&rx);
        // Reserve (both), hold (the task), settle (the task and the project): every change, each with its view.
        assert_eq!(frames.len(), 2 + 1 + 2, "{frames:?}");
        let (_, settled) = frames.last().unwrap();
        assert_eq!(settled["owner"], "pr1");
        assert_eq!(settled["spent"], usd(0.5));
        assert!(settled.get("left").is_some() && settled.get("spent_pct").is_some());
        b.raise(OwnerKind::Task, "t1", Some(usd(1.0)), None, 5);
        b.set_paused(OwnerKind::Project, "pr1", true, 6);
        b.end_task("t1", 7);
        assert_eq!(
            budget_frames(&rx).len(),
            1 + 1 + 2,
            "a raise, a pause and the end of the task"
        );
    }

    #[test]
    fn a_unit_cannot_change_once_the_budget_has_spend_in_it() {
        let b = Budgets::default();
        b.set_project("pr1", Unit::Usd, Some(usd(10.0)), 80, None, None, 1)
            .unwrap();
        b.set_task("t1", "pr1", Unit::Usd, Some(usd(4.0)), 1)
            .unwrap();
        // No spend yet: the unit changes.
        b.set_task("t1", "pr1", Unit::Tokens, Some(2_000_000), 1)
            .unwrap();
        b.set_task("t1", "pr1", Unit::Usd, Some(usd(4.0)), 1)
            .unwrap();
        b.start_task("t1", "pr1", usd(4.0), 2).unwrap();
        // Reserved: the task's unit is locked, and so is its project's.
        let refused = Refused::UnitLocked { unit: Unit::Usd };
        assert_eq!(
            b.set_task("t1", "pr1", Unit::Tokens, Some(2_000_000), 3),
            Err(refused.clone())
        );
        assert_eq!(
            b.set_project("pr1", Unit::Tokens, None, 80, None, None, 3),
            Err(refused.clone())
        );
        assert_eq!(
            refused.to_string(),
            "this budget has spend in usd; its unit can no longer change"
        );
        // The same unit still sets the rest (the cap, the warning share).
        b.set_project("pr1", Unit::Usd, Some(usd(12.0)), 50, None, None, 3)
            .unwrap();
        assert_eq!(
            b.get(OwnerKind::Project, "pr1").unwrap().cap,
            Some(usd(12.0))
        );
        // A task with spend in its own unit: the same refusal, in the unit it has.
        let hold = b
            .hold(Some("t1"), "pr1", Amount::of(usd(0.5), 0), None, None, 4)
            .unwrap();
        b.settle(&hold, Amount::of(usd(0.5), 0), None, None, 5);
        b.end_task("t1", 6);
        assert_eq!(
            b.set_task("t1", "pr1", Unit::Tokens, None, 7),
            Err(refused),
            "a charge stays: the task's unit is locked after it ends"
        );
    }
}
