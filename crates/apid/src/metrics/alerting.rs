//! Alert rules on the hub: the starter rules plus the owner's, their states, mutes, acknowledgements and history,
//! saved through the store (`alerts` saved value). Checks run on the tick against the open 1-minute windows (and,
//! for daily sums, the day so far); a transition is published (`metrics.alert` on the bus, which the consoles turn
//! into a banner and a desktop notification), recorded as an `alert` event, and optionally posted to the board.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

use apz_metrics::alerts::{
    Rule, State, Tracker, Transition, slo_status, starter_rules, starter_slos,
};
use apz_metrics::query::AggFn;
use apz_metrics::{EntityKind, Event, SeriesKey};
use apz_metrics_view::source::{AlertsState, GroupBy, Read, Source};

use super::Metrics;
use super::ingest;
use super::scope;
use super::source::LiveSource;
use super::store::Store;
use crate::state::App;

/// Saved alert state.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    /// The owner's rules and the starters' on/off.
    pub rules: Vec<Rule>,
    pub tracker: Tracker,
    pub muted: Vec<(String, u64)>,
    pub acked: Vec<(String, String)>,
    /// Newest first, at most 500.
    pub history: Vec<(u64, String, String, String, f64, Option<u64>)>,
    /// The latest value of each (rule, subject) key. Not saved: the next check fills it again.
    #[serde(skip)]
    pub values: HashMap<String, f64>,
}

/// One line of the alert history: (time, rule id, subject, what, value, lasted ms).
type HistoryEntry = (u64, String, String, String, f64, Option<u64>);

/// How many history entries are kept.
const HISTORY_LIMIT: usize = 500;
/// The window a reading of a rule looks back over, when it is not a daily sum.
const WINDOW_MS: u64 = 60_000;
/// The width of the windows a service level is counted in.
const SLO_WINDOW_MS: u64 = 300_000;
const DAY_MS: u64 = 86_400_000;
/// Separates the rule id and the subject in the tracker's keys.
const KEY_SEP: char = '\u{1f}';

/// The saved alert state as its table (`metric_alerts`, one row): what [`Saved`] keeps, with each list as child rows.
#[derive(Clone, Debug, Default, PartialEq, apz_store::Table)]
#[table(name = "metric_alerts")]
pub struct Stored {
    #[table(child)]
    pub rules: Vec<Rule>,
    #[table(map)]
    pub states: HashMap<String, State>,
    #[table(child)]
    pub muted: Vec<Mute>,
    #[table(child)]
    pub acked: Vec<Ack>,
    #[table(child)]
    pub history: Vec<HistoryRow>,
}

/// A rule muted until a time.
#[derive(Clone, Debug, PartialEq, apz_store::Table)]
pub struct Mute {
    pub rule: String,
    pub until_ms: u64,
}

/// A subject of a rule acknowledged while it fires.
#[derive(Clone, Debug, PartialEq, apz_store::Table)]
pub struct Ack {
    pub rule: String,
    pub subject: String,
}

/// One line of the alert history.
#[derive(Clone, Debug, PartialEq, apz_store::Table)]
pub struct HistoryRow {
    pub at_ms: u64,
    pub rule: String,
    pub subject: String,
    pub what: String,
    pub value: f64,
    pub lasted_ms: Option<u64>,
}

impl Stored {
    fn from_saved(saved: &Saved) -> Stored {
        Stored {
            rules: saved.rules.clone(),
            states: saved.tracker.states.clone(),
            muted: saved
                .muted
                .iter()
                .map(|(rule, until_ms)| Mute {
                    rule: rule.clone(),
                    until_ms: *until_ms,
                })
                .collect(),
            acked: saved
                .acked
                .iter()
                .map(|(rule, subject)| Ack {
                    rule: rule.clone(),
                    subject: subject.clone(),
                })
                .collect(),
            history: saved
                .history
                .iter()
                .map(|(at_ms, rule, subject, what, value, lasted)| HistoryRow {
                    at_ms: *at_ms,
                    rule: rule.clone(),
                    subject: subject.clone(),
                    what: what.clone(),
                    value: *value,
                    lasted_ms: *lasted,
                })
                .collect(),
        }
    }

    fn into_saved(self) -> Saved {
        Saved {
            rules: self.rules,
            tracker: Tracker {
                states: self.states,
            },
            muted: self
                .muted
                .into_iter()
                .map(|m| (m.rule, m.until_ms))
                .collect(),
            acked: self
                .acked
                .into_iter()
                .map(|a| (a.rule, a.subject))
                .collect(),
            history: self
                .history
                .into_iter()
                .map(|h| (h.at_ms, h.rule, h.subject, h.what, h.value, h.lasted_ms))
                .collect(),
            values: HashMap::new(),
        }
    }
}

pub struct Alerts {
    inner: Mutex<Saved>,
}

impl Alerts {
    /// Loads saved alerts, adding any starter rule not yet saved.
    pub fn load(store: &dyn Store) -> Alerts {
        let mut saved = store.alerts().map(Stored::into_saved).unwrap_or_default();
        for starter in starter_rules() {
            if !saved.rules.iter().any(|rule| rule.id == starter.id) {
                saved.rules.push(starter);
            }
        }
        Alerts {
            inner: Mutex::new(saved),
        }
    }

    pub fn rules(&self) -> Vec<Rule> {
        super::lock(&self.inner).rules.clone()
    }

    /// Adds or replaces a rule (a new id when empty). A starter rule may only be switched on or off.
    pub fn put_rule(&self, store: &dyn Store, rule: Rule) -> Result<Rule, String> {
        let mut rule = rule;
        let mut saved = super::lock(&self.inner);
        if rule.id.is_empty() {
            rule.id = next_rule_id(&saved.rules);
        }
        let stored = match saved
            .rules
            .iter()
            .position(|existing| existing.id == rule.id)
        {
            Some(index) if saved.rules[index].starter => {
                saved.rules[index].on = rule.on;
                saved.rules[index].clone()
            }
            Some(index) => {
                // Only the starters are built in; an owner's rule is never one.
                rule.starter = false;
                saved.rules[index] = rule.clone();
                rule
            }
            None => {
                rule.starter = false;
                saved.rules.push(rule.clone());
                rule
            }
        };
        save(store, &saved);
        Ok(stored)
    }

    /// Deletes an owner's rule (starters cannot be deleted).
    pub fn delete(&self, store: &dyn Store, id: &str) -> Result<(), String> {
        let mut saved = super::lock(&self.inner);
        let index = saved
            .rules
            .iter()
            .position(|rule| rule.id == id)
            .ok_or_else(|| "no such rule".to_string())?;
        if saved.rules[index].starter {
            return Err("a starter rule cannot be deleted".to_string());
        }
        saved.rules.remove(index);
        forget_rule(&mut saved, id);
        save(store, &saved);
        Ok(())
    }

    pub fn mute(&self, store: &dyn Store, id: &str, until_ms: u64, now_ms: u64) {
        let mut saved = super::lock(&self.inner);
        saved.muted.retain(|(rule, _)| rule != id);
        saved.muted.push((id.to_string(), until_ms));
        push_history(
            &mut saved,
            (
                now_ms,
                id.to_string(),
                String::new(),
                "muted".to_string(),
                0.0,
                None,
            ),
        );
        save(store, &saved);
    }

    pub fn ack(&self, store: &dyn Store, id: &str, subject: &str, now_ms: u64) {
        let mut saved = super::lock(&self.inner);
        if !saved
            .acked
            .iter()
            .any(|(rule, acked)| rule == id && acked == subject)
        {
            saved.acked.push((id.to_string(), subject.to_string()));
        }
        let value = saved
            .values
            .get(&pair_key(id, subject))
            .copied()
            .unwrap_or(0.0);
        push_history(
            &mut saved,
            (
                now_ms,
                id.to_string(),
                subject.to_string(),
                "acknowledged".to_string(),
                value,
                None,
            ),
        );
        save(store, &saved);
    }

    /// Checks every rule that is on against current values, recording transitions.
    pub fn check(&self, m: &Metrics, app: &App, now_ms: u64) -> Vec<Transition> {
        let started = Instant::now();
        let src = LiveSource { app, m, now_ms };
        let mut notify = Vec::new();
        let mut events = Vec::new();
        {
            let mut saved = super::lock(&self.inner);
            let rules: Vec<Rule> = saved.rules.iter().filter(|rule| rule.on).cloned().collect();
            let mut changed = false;
            let mut present: HashSet<String> = HashSet::new();
            for rule in &rules {
                for subject in subjects(app, &rule.scope) {
                    present.insert(pair_key(&rule.id, &subject));
                    let Some(value) = src.scalar(
                        &rule.metric,
                        &subject,
                        window_start(rule, now_ms),
                        now_ms,
                        rule.agg,
                    ) else {
                        continue;
                    };
                    saved.values.insert(pair_key(&rule.id, &subject), value);
                    let before = saved.tracker.state(&rule.id, &subject);
                    let transition = saved.tracker.observe(rule, &subject, value, now_ms);
                    if saved.tracker.state(&rule.id, &subject) != before {
                        changed = true;
                    }
                    let Some(transition) = transition else {
                        continue;
                    };
                    let lasted = match before {
                        State::Firing { since_ms } | State::Clearing { since_ms, .. } => {
                            Some(now_ms.saturating_sub(since_ms))
                        }
                        _ => None,
                    };
                    push_history(
                        &mut saved,
                        (
                            now_ms,
                            rule.id.clone(),
                            subject.clone(),
                            transition.what.clone(),
                            value,
                            lasted,
                        ),
                    );
                    if transition.what == "resolved" {
                        saved
                            .acked
                            .retain(|(rule_id, acked)| !(rule_id == &rule.id && acked == &subject));
                    }
                    events.push(Event {
                        t_ms: now_ms,
                        kind: "alert".to_string(),
                        entity_kind: EntityKind::Global,
                        entity: "global".to_string(),
                        duration_ms: lasted.map(|ms| ms as f64),
                        detail: format!("{}: {} on {}", transition.what, rule.name, subject),
                        parts: vec![("value".to_string(), value)],
                    });
                    if !is_muted(&saved, &rule.id, now_ms) {
                        notify.push(transition);
                    }
                }
            }
            keep_present_values(&mut saved.values, &present);
            if changed {
                save(m.store.as_ref(), &saved);
            }
        }
        for event in events {
            ingest::record_event(m, event, "");
        }
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        m.registry
            .hist(SeriesKey::new("self.alert_eval_ms", EntityKind::Hub, "hub"))
            .record(elapsed_ms);
        notify
    }

    /// What the Alerts tab shows.
    pub fn state(&self, m: &Metrics, app: &App, now_ms: u64) -> AlertsState {
        // Copied out so the store and the series reads run without the lock held.
        let saved = (*super::lock(&self.inner)).clone();
        let src = LiveSource { app, m, now_ms };
        let mut rows: Vec<(String, String, State, f64)> = saved
            .tracker
            .states
            .iter()
            .filter_map(|(key, state)| {
                let (rule, subject) = key.split_once(KEY_SEP)?;
                let value = saved.values.get(key).copied().unwrap_or(0.0);
                Some((rule.to_string(), subject.to_string(), *state, value))
            })
            .collect();
        rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let muted = saved
            .muted
            .iter()
            .filter(|(_, until)| *until > now_ms)
            .cloned()
            .collect();
        let month_start = utc_month_start(now_ms);
        let slos = starter_slos()
            .into_iter()
            .map(|slo| {
                let read = Read {
                    metric: slo.metric.clone(),
                    scope: slo.scope.clone(),
                    from_ms: month_start,
                    to_ms: now_ms,
                    step_ms: SLO_WINDOW_MS,
                    agg: slo.agg,
                    group: GroupBy::All,
                    top: None,
                };
                let windows: Vec<f64> = src
                    .series(&read)
                    .into_iter()
                    .flat_map(|group| group.values)
                    .flatten()
                    .collect();
                let status = slo_status(&slo, &windows);
                (slo, status)
            })
            .collect();
        AlertsState {
            rules: saved.rules,
            subjects: rows,
            muted,
            acked: saved.acked,
            history: saved.history,
            slos,
        }
    }
}

/// The id of the next owner's rule: `rule-` and one more than the largest numeric suffix in use.
fn next_rule_id(rules: &[Rule]) -> String {
    let largest = rules
        .iter()
        .filter_map(|rule| rule.id.rsplit('-').next()?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("rule-{}", largest + 1)
}

/// The key of a (rule, subject) pair, as the tracker keys it.
fn pair_key(rule: &str, subject: &str) -> String {
    format!("{rule}{KEY_SEP}{subject}")
}

/// Keeps the latest value only of the (rule, subject) pairs that exist now. A subject that went away (a VM
/// stopped, an agent deleted) takes its value with it, so the map follows the subjects rather than growing with
/// every subject ever seen.
fn keep_present_values(values: &mut HashMap<String, f64>, present: &HashSet<String>) {
    values.retain(|key, _| present.contains(key));
}

/// Writes the saved state through the store.
fn save(store: &dyn Store, saved: &Saved) {
    store.put_alerts(&Stored::from_saved(saved));
}

/// Puts an entry first in the history, keeping the newest entries.
fn push_history(saved: &mut Saved, entry: HistoryEntry) {
    saved.history.insert(0, entry);
    saved.history.truncate(HISTORY_LIMIT);
}

/// Drops everything kept about a rule: its states, latest values, mutes and acknowledgements.
fn forget_rule(saved: &mut Saved, id: &str) {
    let prefix = pair_key(id, "");
    saved
        .tracker
        .states
        .retain(|key, _| !key.starts_with(&prefix));
    saved.values.retain(|key, _| !key.starts_with(&prefix));
    saved.muted.retain(|(rule, _)| rule != id);
    saved.acked.retain(|(rule, _)| rule != id);
}

/// Whether a rule is muted at a time.
fn is_muted(saved: &Saved, rule: &str, now_ms: u64) -> bool {
    saved
        .muted
        .iter()
        .any(|(id, until)| id == rule && *until > now_ms)
}

/// The subjects a rule is checked on: its scope, or each entity of the kind for an "every …" scope.
fn subjects(app: &App, target: &str) -> Vec<String> {
    match target.strip_prefix("every:") {
        Some(kind) => scope::entities(app, "global", kind)
            .into_iter()
            .map(|entity| entity.id)
            .collect(),
        None => vec![target.to_string()],
    }
}

/// Where a rule's reading starts: the start of the UTC day for a daily spend sum, else the last minute.
fn window_start(rule: &Rule, now_ms: u64) -> u64 {
    if rule.agg == AggFn::Sum && rule.metric.contains("spend") {
        now_ms - now_ms % DAY_MS
    } else {
        now_ms.saturating_sub(WINDOW_MS)
    }
}

/// The start of the UTC month that holds `now_ms`.
fn utc_month_start(now_ms: u64) -> u64 {
    let days = (now_ms / DAY_MS) as i64;
    (days - (day_of_month(days) - 1)) as u64 * DAY_MS
}

/// The day of the month (1 to 31) of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn day_of_month(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_based_month = (5 * day_of_year + 2) / 153;
    day_of_year - (153 * march_based_month + 2) / 5 + 1
}

#[cfg(test)]
mod tests {
    use apz_metrics::alerts::{Cmp, Notify, Severity};
    use apz_metrics::query::AggFn;

    use std::sync::Arc;

    use super::*;
    use crate::metrics::sqlite::{SqliteStore, scratch_store};

    /// A new owner's rule with no id yet.
    fn owner_rule() -> Rule {
        Rule {
            id: String::new(),
            name: "Slow first token".to_string(),
            metric: "llm.ttft_ms".to_string(),
            agg: AggFn::P95,
            scope: "global".to_string(),
            cmp: Cmp::Gt,
            threshold: 5000.0,
            for_ms: 0,
            severity: Severity::Warning,
            on: true,
            notify: Notify::default(),
            starter: false,
        }
    }

    #[test]
    fn loading_adds_the_starter_rules_and_a_starter_only_changes_its_switch() {
        let files = scratch_store("alerts-starters");
        let store = SqliteStore::new(Arc::clone(files.db(crate::store::METRICS)));
        let alerts = Alerts::load(&store);
        let rules = alerts.rules();
        assert_eq!(rules.len(), starter_rules().len());
        assert!(rules.iter().all(|rule| rule.starter));

        let mut starter = rules
            .iter()
            .find(|rule| rule.starter)
            .cloned()
            .expect("a starter");
        let threshold = starter.threshold;
        starter.threshold = threshold * 10.0;
        starter.on = false;
        let stored = alerts
            .put_rule(&store, starter.clone())
            .expect("the switch changes");
        assert_eq!(stored.threshold, threshold);
        assert!(!stored.on);
        let kept = alerts
            .rules()
            .into_iter()
            .find(|rule| rule.id == starter.id)
            .expect("still there");
        assert!(!kept.on);
        assert_eq!(kept.threshold, threshold);
    }

    #[test]
    fn delete_refuses_a_starter_and_removes_an_owners_rule() {
        let files = scratch_store("alerts-delete");
        let store = SqliteStore::new(Arc::clone(files.db(crate::store::METRICS)));
        let alerts = Alerts::load(&store);
        let starter_id = alerts.rules()[0].id.clone();
        assert_eq!(
            alerts.delete(&store, &starter_id),
            Err("a starter rule cannot be deleted".to_string())
        );

        let owner = alerts.put_rule(&store, owner_rule()).expect("a new rule");
        assert!(owner.id.starts_with("rule-"));
        assert_eq!(alerts.delete(&store, &owner.id), Ok(()));
        assert!(alerts.rules().iter().all(|rule| rule.id != owner.id));
        assert_eq!(
            alerts.delete(&store, &owner.id),
            Err("no such rule".to_string())
        );
    }

    #[test]
    fn a_value_is_kept_only_while_its_subject_is_present() {
        let mut values: HashMap<String, f64> = HashMap::new();
        values.insert(pair_key("cpu", "node-a"), 41.0);
        values.insert(pair_key("cpu", "vm-stopped"), 7.0);
        let present: HashSet<String> = [pair_key("cpu", "node-a")].into_iter().collect();
        keep_present_values(&mut values, &present);
        assert_eq!(values.len(), 1);
        assert_eq!(values.get(&pair_key("cpu", "node-a")), Some(&41.0));
    }
}
