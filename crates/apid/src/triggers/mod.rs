//! Triggers (PLAN 14.6, 5.2): what starts a recipe run. A trigger is a typed record per recipe (`recipe_trigger`): an
//! event from a source, or a schedule (a cron expression, or an interval in minutes), or the owner's hand (`routes.rs`).
//!
//! The sources: a connector's webhook delivery (`hooks.rs`); Weft's event feed (`weft_event`: a lane opened, a change
//! landed, a release proposed); a CI run that ended (`ci_run_ended`); a maintenance signal (`maintenance_signal`). Each
//! takes its event with [`Triggers::deliver`], and an event with an identity of its own (Weft's sequence number, a run's
//! id, a signal's id) is taken once per trigger, so the same landing or run never starts a trigger twice.
//!
//! One scheduler owns the schedules: [`tick`] runs on the scheduler's pass and fires the schedules that are due. The
//! full suite of a project (`full_run.rs`) and the engine watch (`engine_watch.rs`) are schedule triggers too.
//!
//! Deliveries are debounced: a delivery opens a window of the trigger's `window_ms`, and every delivery of the same item
//! inside it is absorbed, so a burst is one run. A run is deduplicated: one run per item and recipe at a time; a
//! delivery that comes while the item's run is still going waits and starts a run when that one ends. The windows and
//! the taken identities are rows in plaza.db (`recipe_trigger_pending`, `recipe_trigger_fired`), so a restart keeps
//! both: a window that was open is still open, and a replayed event is still taken.

pub mod cron;
pub mod hooks;
pub mod routes;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recipes::runs::{RunState, Runs};
use crate::state::App;

/// The vault binding the webhook tokens and signing secrets are stored under (`hub:<id>`, the hub's own consumers).
pub const BINDING: &str = "hub:triggers";
/// The window a trigger waits when its record names none: a minute.
pub const DEFAULT_WINDOW_MS: u64 = 60_000;
/// The engine watch's record id and recipe.
pub const ENGINE_WATCH: &str = "engine-watch";
/// How long a taken identity is kept: a replay of an event older than this would take its trigger again.
const FIRED_KEEP_MS: u64 = 30 * 24 * 3600 * 1000;

/// What a trigger does when it fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Starts a run of the trigger's recipe (its engine steps; the recipe's own steps are the run's).
    Run,
    /// The scheduled full suite of the trigger's project (4.4): its coverage map is recorded again.
    FullSuite,
    /// The engine watch (12.6).
    EngineWatch,
}

/// One trigger: a record per recipe, in plaza.db (`recipe_trigger`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "recipe_trigger")]
pub struct TriggerRow {
    #[table(key)]
    pub id: String,
    pub recipe: String,
    /// The project the run is for (the full suite's project; a recipe's run for a project).
    pub project: Option<String>,
    pub action: Action,
    /// Where the event comes from: `connector:<id>`, `weft`, `ci`, `maintenance`, or `schedule` / `owner`.
    pub source: String,
    /// The event's name (`change.landed`); empty for a schedule.
    pub event: String,
    /// A filter on the event's label; empty takes any.
    pub label: String,
    /// A cron expression (five fields, UTC); empty when the schedule is an interval or the trigger is an event.
    pub cron: String,
    /// An interval in minutes (0: none). With `cron` empty, the next fire is this long after the last.
    pub every_minutes: u32,
    /// The debounce window in ms: a burst of deliveries of one item within it is one run.
    pub window_ms: u64,
    pub enabled: bool,
    /// When a schedule next fires (Unix ms).
    pub next_ms: Option<u64>,
    /// When a schedule last fired (Unix ms).
    pub last_ms: Option<u64>,
    pub created_ms: u64,
}

impl TriggerRow {
    /// A schedule's next fire after `from`: the cron expression's, or the interval's. None when there is none.
    pub fn next_after(&self, from: u64) -> Option<u64> {
        if !self.cron.is_empty() {
            return cron::parse(&self.cron).ok()?.next_after(from);
        }
        (self.every_minutes > 0).then(|| from + u64::from(self.every_minutes) * 60_000)
    }

    /// Whether this record is a schedule (it fires by the clock, not by an event).
    pub fn is_schedule(&self) -> bool {
        !self.cron.is_empty() || self.every_minutes > 0
    }
}

/// One event from a source: a connector's delivery, a Weft event, a CI run, a maintenance signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub source: String,
    pub event: String,
    /// The item the event is about (an issue, a change, a project): the key of the debounce and of the run's subject.
    pub item: String,
    pub label: String,
    /// The event's own identity: a trigger takes each identity once. None for a delivery with none of its own.
    pub dedup: Option<String>,
}

/// A delivery waiting out its window, by (trigger, item). Kept in plaza.db (`recipe_trigger_pending`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "recipe_trigger_pending")]
pub struct PendingRow {
    #[table(key)]
    pub trigger: String,
    #[table(key)]
    pub item: String,
    pub recipe: String,
    pub project: Option<String>,
    pub source: String,
    pub fire_at: u64,
    pub deliveries: u32,
}

/// An identity a trigger has taken (`recipe_trigger_fired`): a replay of the event starts nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "recipe_trigger_fired")]
pub struct FiredRow {
    #[table(key)]
    pub trigger: String,
    #[table(key)]
    pub identity: String,
    pub fired_ms: u64,
}

#[derive(Default)]
struct Inner {
    rows: BTreeMap<String, TriggerRow>,
    pending: BTreeMap<(String, String), PendingRow>,
    fired: BTreeMap<(String, String), FiredRow>,
    next: u64,
}

/// The triggers, the deliveries waiting on their windows, and the identities taken.
#[derive(Default)]
pub struct Triggers {
    inner: Mutex<Inner>,
}

/// The table of the triggers: `recipe_trigger`, one row per trigger.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(TriggerRow)
}

/// The table of the open windows.
pub fn pending_def() -> &'static TableDef {
    apz_store::root_def!(PendingRow)
}

/// The table of the identities taken.
pub fn fired_def() -> &'static TableDef {
    apz_store::root_def!(FiredRow)
}

/// Whether a recipe's steps all run on an engine, so a trigger can start it (the others wait for a task's seats).
pub fn engine_only(runs: &Runs, recipe: &str) -> bool {
    runs.recipe(recipe, None)
        .is_some_and(|r| !r.steps.is_empty() && r.steps.iter().all(|s| s.engine.is_some()))
}

/// Whether a run of `recipe` for `item` is still going (one run per item and recipe at a time).
fn running_for(runs: &Runs, recipe: &str, item: &str) -> bool {
    runs.list(None, None).iter().any(|v| {
        v.run.recipe == recipe
            && v.run.subject.as_deref().unwrap_or("") == item
            && matches!(v.run.state, RunState::Running | RunState::Owner)
    })
}

impl Triggers {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The triggers' rows for the hub's store.
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let inner = self.lock();
        let rows: Vec<TriggerRow> = inner.rows.values().cloned().collect();
        RowSet::list(def(), rows.iter())
    }

    /// Replaces the triggers with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<TriggerRow> = apz_store::table::load_list(conn, def())?;
        let mut inner = self.lock();
        inner.next = stored
            .iter()
            .filter_map(|t| t.id.strip_prefix("rt-")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        inner.rows = stored.into_iter().map(|t| (t.id.clone(), t)).collect();
        Ok(())
    }

    /// The open windows, for the hub's store.
    pub fn pending_rows(&self) -> Result<RowSet, StoreError> {
        let inner = self.lock();
        let rows: Vec<PendingRow> = inner.pending.values().cloned().collect();
        RowSet::list(pending_def(), rows.iter())
    }

    /// Replaces the open windows with what plaza.db holds (at start): a restart keeps each window's fire time.
    pub fn load_pending(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<PendingRow> = apz_store::table::load_list(conn, pending_def())?;
        self.lock().pending = stored
            .into_iter()
            .map(|p| ((p.trigger.clone(), p.item.clone()), p))
            .collect();
        Ok(())
    }

    /// The identities taken, for the hub's store.
    pub fn fired_rows(&self) -> Result<RowSet, StoreError> {
        let inner = self.lock();
        let rows: Vec<FiredRow> = inner.fired.values().cloned().collect();
        RowSet::list(fired_def(), rows.iter())
    }

    /// Replaces the identities taken with what plaza.db holds (at start).
    pub fn load_fired(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<FiredRow> = apz_store::table::load_list(conn, fired_def())?;
        self.lock().fired = stored
            .into_iter()
            .map(|f| ((f.trigger.clone(), f.identity.clone()), f))
            .collect();
        Ok(())
    }

    /// The triggers of a recipe, oldest first.
    pub fn of_recipe(&self, recipe: &str) -> Vec<TriggerRow> {
        let inner = self.lock();
        let mut out: Vec<TriggerRow> = inner
            .rows
            .values()
            .filter(|t| t.recipe == recipe)
            .cloned()
            .collect();
        out.sort_by_key(|t| t.created_ms);
        out
    }

    /// A trigger by its id.
    pub fn get(&self, id: &str) -> Option<TriggerRow> {
        self.lock().rows.get(id).cloned()
    }

    /// Adds a trigger the owner made. Its id is a new `rt-<n>`; a schedule's next fire is set from `now`.
    pub fn add(&self, mut row: TriggerRow, now: u64) -> TriggerRow {
        let mut inner = self.lock();
        inner.next += 1;
        row.id = format!("rt-{}", inner.next);
        row.created_ms = now;
        row.next_ms = row.is_schedule().then(|| row.next_after(now)).flatten();
        row.enabled = true;
        inner.rows.insert(row.id.clone(), row.clone());
        row
    }

    /// Removes a trigger. True when there was one.
    pub fn remove(&self, id: &str) -> bool {
        self.lock().rows.remove(id).is_some()
    }

    /// Keeps a trigger a plaza component makes for itself under its own id (intake's rule triggers): it is added when
    /// there is none under that id, and left as it is when there is.
    pub fn ensure(&self, row: TriggerRow) {
        self.lock().rows.entry(row.id.clone()).or_insert(row);
    }

    /// Takes an event: every enabled trigger that matches it (its source, its event, and its label when it names one)
    /// takes it, unless the trigger has taken its identity already, and opens its window for the item or joins the
    /// window already open. Returns how many triggers took it.
    pub fn deliver(&self, event: &Event, now: u64) -> usize {
        let mut inner = self.lock();
        let matched: Vec<TriggerRow> = inner
            .rows
            .values()
            .filter(|t| {
                t.enabled
                    && !t.is_schedule()
                    && t.source == event.source
                    && t.event == event.event
                    && (t.label.is_empty() || t.label == event.label)
            })
            .cloned()
            .collect();
        let mut taken = 0;
        for t in &matched {
            if let Some(identity) = &event.dedup {
                let key = (t.id.clone(), identity.clone());
                if inner.fired.contains_key(&key) {
                    continue;
                }
                inner.fired.insert(
                    key,
                    FiredRow {
                        trigger: t.id.clone(),
                        identity: identity.clone(),
                        fired_ms: now,
                    },
                );
            }
            taken += 1;
            inner
                .pending
                .entry((t.id.clone(), event.item.clone()))
                .and_modify(|p| p.deliveries += 1)
                .or_insert_with(|| PendingRow {
                    trigger: t.id.clone(),
                    item: event.item.clone(),
                    recipe: t.recipe.clone(),
                    project: t.project.clone(),
                    source: t.source.clone(),
                    fire_at: now + t.window_ms,
                    deliveries: 1,
                });
        }
        taken
    }

    /// The enabled schedules due at `now`.
    pub fn due(&self, now: u64) -> Vec<TriggerRow> {
        self.lock()
            .rows
            .values()
            .filter(|t| t.enabled && t.next_ms.is_some_and(|at| at <= now))
            .cloned()
            .collect()
    }

    /// A schedule fired at `now`: its last fire is `now` and its next one is set. A schedule with no next fire is off.
    pub fn advanced(&self, id: &str, now: u64) {
        if let Some(row) = self.lock().rows.get_mut(id) {
            row.last_ms = Some(now);
            row.next_ms = row.next_after(now);
            row.enabled = row.next_ms.is_some();
        }
    }

    /// A schedule of the scheduler's own fires at `now`: it waits for no window. Its item is empty.
    pub fn queue_schedule(&self, row: &TriggerRow, now: u64) {
        self.lock()
            .pending
            .entry((row.id.clone(), String::new()))
            .or_insert_with(|| PendingRow {
                trigger: row.id.clone(),
                item: String::new(),
                recipe: row.recipe.clone(),
                project: row.project.clone(),
                source: row.source.clone(),
                fire_at: now,
                deliveries: 1,
            });
    }

    /// Starts the runs of the deliveries whose windows have closed, one per item and recipe at a time, and forgets the
    /// identities taken long ago. Returns the runs started.
    pub fn start_due(&self, runs: &Runs, now: u64) -> Vec<String> {
        let mut inner = self.lock();
        inner.fired.retain(|_, f| f.fired_ms + FIRED_KEEP_MS >= now);
        let due: Vec<(String, String)> = inner
            .pending
            .iter()
            .filter(|(_, p)| p.fire_at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        let mut started = Vec::new();
        for key in due {
            let Some(pending) = inner.pending.get(&key).cloned() else {
                continue;
            };
            let item = key.1.clone();
            if running_for(runs, &pending.recipe, &item) {
                continue;
            }
            inner.pending.remove(&key);
            // Only a recipe whose steps all run on an engine can be started here: a role's step waits for a task's seat,
            // and nothing fills it from a trigger.
            if !engine_only(runs, &pending.recipe) {
                eprintln!(
                    "apid: trigger {} starts no run: recipe {} has steps that wait for a task",
                    key.0, pending.recipe
                );
                continue;
            }
            let subject = (!item.is_empty()).then_some(item.as_str());
            match runs.start_engine(
                &pending.recipe,
                &pending.source,
                None,
                pending.project.as_deref(),
                subject,
                now,
            ) {
                Some(run) => started.push(run),
                None => eprintln!(
                    "apid: trigger {} starts no run: no recipe {}",
                    key.0, pending.recipe
                ),
            }
        }
        started
    }

    /// Keeps a project's full-suite schedule (4.4) in step with its settings and its map: an interval of `minutes`
    /// after the last run or the last map, now when neither exists. A project whose full run is off or not eligible has
    /// the schedule off.
    pub fn keep_full_suite(
        &self,
        project: &str,
        minutes: u32,
        enabled: bool,
        mapped: Option<u64>,
        now: u64,
    ) {
        let id = format!("full-suite:{project}");
        let mut inner = self.lock();
        let row = inner.rows.entry(id.clone()).or_insert_with(|| TriggerRow {
            id,
            recipe: "ci".into(),
            project: Some(project.to_string()),
            action: Action::FullSuite,
            source: "schedule".into(),
            event: String::new(),
            label: String::new(),
            cron: String::new(),
            every_minutes: minutes,
            window_ms: 0,
            enabled,
            next_ms: None,
            last_ms: None,
            created_ms: now,
        });
        row.every_minutes = minutes;
        row.enabled = enabled && minutes > 0;
        row.last_ms = row.last_ms.max(mapped);
        row.next_ms = Some(
            row.last_ms
                .map_or(now, |last| last + u64::from(minutes) * 60_000),
        );
    }

    /// Sets the engine watch's schedule for this start: its first run a minute from now, then its interval (12.6).
    pub fn start_engine_watch(&self, now: u64) {
        let mut inner = self.lock();
        let row = inner
            .rows
            .entry(ENGINE_WATCH.to_string())
            .or_insert_with(|| TriggerRow {
                id: ENGINE_WATCH.into(),
                recipe: ENGINE_WATCH.into(),
                project: None,
                action: Action::EngineWatch,
                source: "schedule".into(),
                event: String::new(),
                label: String::new(),
                cron: String::new(),
                every_minutes: crate::engine_watch::EVERY_MINUTES,
                window_ms: 0,
                enabled: true,
                next_ms: None,
                last_ms: None,
                created_ms: now,
            });
        row.every_minutes = crate::engine_watch::EVERY_MINUTES;
        row.enabled = true;
        row.next_ms = Some(now + crate::engine_watch::FIRST_RUN_MS);
    }
}

/// Takes an event from one of the plaza's sources, at the wall clock.
pub fn emit(app: &App, event: &Event) -> usize {
    app.triggers.deliver(event, apz_plaza::now_ms())
}

/// A Weft event from the feed `weft_hub.rs` reads (`/v1/events`): a lane opened (`lane.opened`), a change landed
/// (`change.landed`), a release proposed (`release.proposed`). Its identity is the feed's sequence number.
pub fn weft_event(app: &App, seq: u64, kind: &str, body: &Value) -> usize {
    let repo = body["repo"].as_str().unwrap_or("");
    let (event, item) = match kind {
        "lane.open" => (
            "lane.opened",
            body["change"].as_str().map(|c| format!("{repo}#{c}")),
        ),
        "land" => (
            "change.landed",
            body["change"].as_str().map(|c| format!("{repo}#{c}")),
        ),
        "release.propose" => (
            "release.proposed",
            body["version"].as_str().map(|v| format!("{repo}@{v}")),
        ),
        _ => return 0,
    };
    let Some(item) = item.filter(|_| !repo.is_empty()) else {
        return 0;
    };
    emit(
        app,
        &Event {
            source: "weft".into(),
            event: event.into(),
            item,
            label: String::new(),
            dedup: Some(format!("weft:{seq}")),
        },
    )
}

/// A CI run ended (4.5) for a change of a project: the run's identity is taken once.
pub fn ci_run_ended(app: &App, project: &str, change: &str, run: &str) -> usize {
    emit(
        app,
        &Event {
            source: "ci".into(),
            event: "run.ended".into(),
            item: format!("{project}#{change}"),
            label: String::new(),
            dedup: Some(format!("ci:{run}")),
        },
    )
}

/// A maintenance signal arrived for a project (`maintenance.rs`): debounced per project, taken once per signal.
pub fn maintenance_signal(app: &App, project: &str, signal_id: &str, kind: &str) -> usize {
    emit(
        app,
        &Event {
            source: "maintenance".into(),
            event: "signal.received".into(),
            item: project.to_string(),
            label: kind.to_string(),
            dedup: Some(format!("maintenance:{project}:{signal_id}")),
        },
    )
}

/// The scheduler's pass (every 2 s, while the plaza runs): fires the schedules that are due, then starts the runs of the
/// deliveries whose windows closed.
pub fn tick(app: &Arc<App>, now: u64) {
    for row in app.triggers.due(now) {
        let started = match row.action {
            Action::Run => {
                app.triggers.queue_schedule(&row, now);
                true
            }
            Action::FullSuite => {
                let project = row.project.clone().unwrap_or_default();
                crate::full_run::start(app, &project)
            }
            Action::EngineWatch => {
                crate::engine_watch::start(Arc::clone(app));
                true
            }
        };
        if started {
            app.triggers.advanced(&row.id, now);
        }
    }
    app.triggers.start_due(&app.recipe_runs, now);
    crate::upgrades::tick(app, now);
    crate::cves::tick(app, now);
}

/// Starts the triggers the plaza keeps for itself at its start: the engine watch's first run is a minute after it.
pub fn ensure_builtins(app: &App, now: u64) {
    app.triggers.start_engine_watch(now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes::runs::Runs;

    /// Unix ms of a UTC time on 2026-10-23 (the test's fake clock).
    fn day(h: u64, m: u64) -> u64 {
        // 2026-10-23 is day 20 749 after 1970-01-01
        (20_749 * 1440 + h * 60 + m) * 60_000
    }

    fn event(item: &str, label: &str) -> Event {
        Event {
            source: "connector:github".into(),
            event: "issue.labelled".into(),
            item: item.into(),
            label: label.into(),
            dedup: None,
        }
    }

    fn labelled_trigger(t: &Triggers, label: &str, window_ms: u64) -> TriggerRow {
        t.add(
            TriggerRow {
                id: String::new(),
                recipe: "intake".into(),
                project: None,
                action: Action::Run,
                source: "connector:github".into(),
                event: "issue.labelled".into(),
                label: label.into(),
                cron: String::new(),
                every_minutes: 0,
                window_ms,
                enabled: true,
                next_ms: None,
                last_ms: None,
                created_ms: 0,
            },
            day(0, 0),
        )
    }

    #[test]
    fn three_deliveries_of_one_labelled_issue_in_the_window_are_one_run() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        labelled_trigger(&triggers, "agent", 2_000);
        let start = day(9, 0);
        for (i, label) in ["agent", "agent", "agent"].iter().enumerate() {
            assert_eq!(
                triggers.deliver(&event("acme/app#7", label), start + i as u64 * 300),
                1
            );
        }
        // the window is open: nothing starts before it closes
        assert!(triggers.start_due(&runs, start + 1_999).is_empty());
        let started = triggers.start_due(&runs, start + 2_000);
        assert_eq!(started.len(), 1, "a burst inside the window is one run");
        assert_eq!(runs.list(None, None).len(), 1);
        assert_eq!(runs.list(None, None)[0].run.trigger, "connector:github");
        assert_eq!(
            runs.list(None, None)[0].run.subject.as_deref(),
            Some("acme/app#7")
        );
        // the item's run is still running: a new delivery waits for it
        triggers.deliver(&event("acme/app#7", "agent"), start + 5_000);
        assert!(triggers.start_due(&runs, start + 10_000).is_empty());
        assert_eq!(runs.list(None, None).len(), 1);
        // another item is not held back by it
        triggers.deliver(&event("acme/app#8", "agent"), start + 5_000);
        assert_eq!(triggers.start_due(&runs, start + 10_000).len(), 1);
    }

    #[test]
    fn a_label_filter_and_a_different_item_do_not_match_the_trigger() {
        let triggers = Triggers::default();
        labelled_trigger(&triggers, "agent", 0);
        assert_eq!(triggers.deliver(&event("acme/app#1", "bug"), day(9, 0)), 0);
        assert_eq!(
            triggers.deliver(&event("acme/app#1", "agent"), day(9, 0)),
            1
        );
        let other = Event {
            source: "connector:linear".into(),
            ..event("ENG-1", "agent")
        };
        assert_eq!(triggers.deliver(&other, day(9, 0)), 0);
    }

    #[test]
    fn a_schedule_fires_at_its_time_with_a_fake_clock_and_then_the_next_day() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        let row = triggers.add(
            TriggerRow {
                id: String::new(),
                recipe: "intake".into(),
                project: Some("acme".into()),
                action: Action::Run,
                source: "schedule".into(),
                event: String::new(),
                label: String::new(),
                cron: "30 9 * * *".into(),
                every_minutes: 0,
                window_ms: 0,
                enabled: true,
                next_ms: None,
                last_ms: None,
                created_ms: 0,
            },
            day(9, 0),
        );
        assert_eq!(row.next_ms, Some(day(9, 30)));
        assert!(triggers.due(day(9, 29)).is_empty(), "not before its minute");
        let due = triggers.due(day(9, 30));
        assert_eq!(due.len(), 1);
        triggers.queue_schedule(&due[0], day(9, 30));
        triggers.advanced(&row.id, day(9, 30));
        let started = triggers.start_due(&runs, day(9, 30));
        assert_eq!(started.len(), 1);
        assert_eq!(
            runs.list(None, None)[0].run.project.as_deref(),
            Some("acme")
        );
        assert_eq!(runs.list(None, None)[0].run.trigger, "schedule");
        // the next fire is the next day, and nothing fires again the same day
        let next = triggers.get(&row.id).unwrap();
        assert_eq!(next.next_ms, Some(day(9, 30) + 86_400_000));
        assert!(triggers.due(day(12, 0)).is_empty());
    }

    #[test]
    fn an_interval_schedule_fires_each_interval_after_its_last_fire() {
        let triggers = Triggers::default();
        triggers.keep_full_suite("acme", 60, true, None, day(0, 0));
        let id = "full-suite:acme";
        assert_eq!(
            triggers.get(id).unwrap().next_ms,
            Some(day(0, 0)),
            "no map yet: now"
        );
        triggers.advanced(id, day(0, 0));
        assert_eq!(triggers.get(id).unwrap().next_ms, Some(day(1, 0)));
        // a map recorded later moves the next run from the map, as the full run's due check did
        triggers.keep_full_suite("acme", 60, true, Some(day(0, 30)), day(0, 5));
        assert_eq!(triggers.get(id).unwrap().next_ms, Some(day(1, 30)));
        assert!(triggers.due(day(1, 29)).iter().all(|t| t.id != id));
        assert!(triggers.due(day(1, 30)).iter().any(|t| t.id == id));
        // off when the project's full run is off
        triggers.keep_full_suite("acme", 0, true, None, day(2, 0));
        assert!(!triggers.get(id).unwrap().enabled);
    }

    #[test]
    fn the_engine_watch_runs_a_minute_after_a_start_then_daily() {
        let triggers = Triggers::default();
        triggers.start_engine_watch(day(8, 0));
        let row = triggers.get(ENGINE_WATCH).unwrap();
        assert_eq!(row.next_ms, Some(day(8, 0) + 60_000));
        assert_eq!(row.every_minutes, 1440);
        triggers.advanced(ENGINE_WATCH, day(8, 1));
        assert_eq!(
            triggers.get(ENGINE_WATCH).unwrap().next_ms,
            Some(day(8, 1) + 86_400_000)
        );
    }

    #[test]
    fn a_recipe_with_steps_that_wait_for_a_task_starts_no_run_and_the_delivery_is_dropped() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        triggers.add(
            TriggerRow {
                id: String::new(),
                recipe: "change".into(),
                project: None,
                action: Action::Run,
                source: "connector:github".into(),
                event: "issue.labelled".into(),
                label: String::new(),
                cron: String::new(),
                every_minutes: 0,
                window_ms: 0,
                enabled: true,
                next_ms: None,
                last_ms: None,
                created_ms: 0,
            },
            day(0, 0),
        );
        assert_eq!(triggers.of_recipe("change").len(), 1);
        triggers.deliver(&event("acme/app#9", ""), day(9, 0));
        assert!(triggers.start_due(&runs, day(9, 0)).is_empty());
        assert!(runs.list(None, None).is_empty());
        assert!(
            triggers.start_due(&runs, day(9, 0)).is_empty(),
            "the delivery was dropped, not kept"
        );
    }

    /// A Weft-style event of `kind` for change `change` of `repo`, identified by `seq`.
    fn weft(seq: u64, change: &str) -> Event {
        Event {
            source: "weft".into(),
            event: "change.landed".into(),
            item: format!("acme/app#{change}"),
            label: String::new(),
            dedup: Some(format!("weft:{seq}")),
        }
    }

    fn weft_trigger(t: &Triggers, window_ms: u64) -> TriggerRow {
        t.add(
            TriggerRow {
                id: String::new(),
                recipe: "intake".into(),
                project: Some("acme".into()),
                action: Action::Run,
                source: "weft".into(),
                event: "change.landed".into(),
                label: String::new(),
                cron: String::new(),
                every_minutes: 0,
                window_ms,
                enabled: true,
                next_ms: None,
                last_ms: None,
                created_ms: 0,
            },
            day(0, 0),
        )
    }

    /// The triggers' rows, windows and taken identities written to an in-memory plaza.db and loaded into fresh ones,
    /// as a restart does.
    fn restarted(t: &Triggers) -> Triggers {
        let conn = Connection::open_in_memory().unwrap();
        for (def, rows) in [
            (def(), t.rows().unwrap()),
            (pending_def(), t.pending_rows().unwrap()),
            (fired_def(), t.fired_rows().unwrap()),
        ] {
            conn.execute_batch(&def.ddl()).unwrap();
            apz_store::table::replace(&conn, def, &rows).unwrap();
        }
        let loaded = Triggers::default();
        loaded.load(&conn).unwrap();
        loaded.load_pending(&conn).unwrap();
        loaded.load_fired(&conn).unwrap();
        loaded
    }

    #[test]
    fn a_landing_starts_one_run_and_a_replay_of_it_starts_none() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        weft_trigger(&triggers, 0);
        let at = day(9, 0);
        assert_eq!(triggers.deliver(&weft(41, "7"), at), 1);
        assert_eq!(
            triggers.deliver(&weft(41, "7"), at + 10),
            0,
            "the same landing is taken once"
        );
        assert_eq!(triggers.start_due(&runs, at + 10).len(), 1);
        assert_eq!(
            triggers.deliver(&weft(41, "7"), at + 20),
            0,
            "and a replay after the run started"
        );
        assert!(triggers.start_due(&runs, at + 20).is_empty());
        assert_eq!(runs.list(None, None).len(), 1);
        // another landing is another event
        assert_eq!(triggers.deliver(&weft(42, "8"), at + 30), 1);
        assert_eq!(triggers.start_due(&runs, at + 30).len(), 1);
        assert_eq!(runs.list(None, None)[0].run.trigger, "weft");
    }

    #[test]
    fn a_window_and_a_taken_landing_survive_a_restart() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        weft_trigger(&triggers, 2_000);
        let at = day(9, 0);
        assert_eq!(triggers.deliver(&weft(7, "3"), at), 1);
        // the restart comes inside the window: the window is still open, and the second delivery joins it
        let triggers = restarted(&triggers);
        assert_eq!(
            triggers.deliver(&weft(7, "3"), at + 1_000),
            0,
            "the landing was taken before the restart"
        );
        assert!(
            triggers.start_due(&runs, at + 1_999).is_empty(),
            "the window is not over"
        );
        assert_eq!(triggers.start_due(&runs, at + 2_000).len(), 1);
        // a restart after the run does not take the landing again
        let triggers = restarted(&triggers);
        assert_eq!(triggers.deliver(&weft(7, "3"), at + 5_000), 0);
        assert!(triggers.start_due(&runs, at + 60_000).is_empty());
        assert_eq!(runs.list(None, None).len(), 1, "one run in all");
    }

    #[test]
    fn a_burst_inside_a_window_that_a_restart_splits_is_still_one_run() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        labelled_trigger(&triggers, "", 2_000);
        let at = day(9, 0);
        assert_eq!(triggers.deliver(&event("acme/app#7", ""), at), 1);
        let triggers = restarted(&triggers);
        assert_eq!(
            triggers.deliver(&event("acme/app#7", ""), at + 500),
            1,
            "a delivery with no identity joins the window"
        );
        assert_eq!(triggers.start_due(&runs, at + 2_000).len(), 1);
        assert_eq!(runs.list(None, None).len(), 1);
    }

    #[test]
    fn a_taken_identity_is_forgotten_after_its_keep_time() {
        let triggers = Triggers::default();
        let runs = Runs::default();
        weft_trigger(&triggers, 0);
        let at = day(9, 0);
        assert_eq!(triggers.deliver(&weft(9, "1"), at), 1);
        triggers.start_due(&runs, at);
        triggers.start_due(&runs, at + FIRED_KEEP_MS + 1);
        assert_eq!(triggers.deliver(&weft(9, "1"), at + FIRED_KEEP_MS + 1), 1);
    }
}
