//! Alert rules and service levels.
//!
//! A rule is one metric (reduced by an [`AggFn`]) in one scope, compared with a threshold for a duration. The hub
//! checks a rule each time the current window of a series it watches updates; there is no polling loop. A
//! subject (the scope, or each entity for an "every …" scope) moves ok → pending → firing when the condition holds
//! for `for_ms`, and firing → resolved → ok when it stays clear for `for_ms` (so it cannot flap).

use crate::query::AggFn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Gt,
    Lt,
}

impl Cmp {
    pub fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Cmp::Gt => value > threshold,
            Cmp::Lt => value < threshold,
        }
    }
    pub fn sign(self) -> &'static str {
        match self {
            Cmp::Gt => ">",
            Cmp::Lt => "<",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Critical,
}

/// Where an alert is told. Nothing leaves the plaza.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Notify {
    pub console: bool,
    pub desktop: bool,
    pub board: bool,
}

impl Default for Notify {
    fn default() -> Self {
        Notify {
            console: true,
            desktop: true,
            board: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Rule {
    pub id: String,
    pub name: String,
    /// A catalog metric id.
    pub metric: String,
    pub agg: AggFn,
    /// A scope id (`global`, `node:n2`, `model:anthropic/opus`) or `every:<kind>` (`every:node`, `every:agent`,
    /// `every:run`) to check each entity of a kind on its own.
    pub scope: String,
    pub cmp: Cmp,
    pub threshold: f64,
    pub for_ms: u64,
    pub severity: Severity,
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default)]
    #[table(flatten)]
    pub notify: Notify,
    /// One of the starter rules (they can be switched off, not deleted).
    #[serde(default)]
    pub starter: bool,
}

fn yes() -> bool {
    true
}

/// A subject's state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    Ok,
    /// The condition holds, not yet for long enough.
    Pending {
        since_ms: u64,
    },
    Firing {
        since_ms: u64,
    },
    /// Was firing; the condition has been clear since `clear_ms`, not yet for long enough.
    Clearing {
        since_ms: u64,
        clear_ms: u64,
    },
}

/// A change worth recording and telling the owner about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub rule: String,
    pub subject: String,
    /// "fired" or "resolved".
    pub what: String,
    pub value: f64,
    pub at_ms: u64,
}

/// The state of every (rule, subject).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tracker {
    pub states: HashMap<String, State>,
}

impl Tracker {
    /// Feeds one reading of a rule for a subject. Returns a transition when the subject fired or resolved.
    pub fn observe(
        &mut self,
        rule: &Rule,
        subject: &str,
        value: f64,
        now_ms: u64,
    ) -> Option<Transition> {
        let key = format!("{}\u{1f}{}", rule.id, subject);
        let holds = rule.cmp.holds(value, rule.threshold);
        let current = self.states.get(&key).copied().unwrap_or(State::Ok);
        let (next, what) = match current {
            State::Ok => {
                if !holds {
                    (State::Ok, None)
                } else if rule.for_ms == 0 {
                    (State::Firing { since_ms: now_ms }, Some("fired"))
                } else {
                    (State::Pending { since_ms: now_ms }, None)
                }
            }
            State::Pending { since_ms } => {
                if !holds {
                    (State::Ok, None)
                } else if now_ms.saturating_sub(since_ms) >= rule.for_ms {
                    (State::Firing { since_ms: now_ms }, Some("fired"))
                } else {
                    (current, None)
                }
            }
            State::Firing { since_ms } => {
                if holds {
                    (current, None)
                } else if rule.for_ms == 0 {
                    (State::Ok, Some("resolved"))
                } else {
                    (
                        State::Clearing {
                            since_ms,
                            clear_ms: now_ms,
                        },
                        None,
                    )
                }
            }
            State::Clearing { since_ms, clear_ms } => {
                if holds {
                    (State::Firing { since_ms }, None)
                } else if now_ms.saturating_sub(clear_ms) >= rule.for_ms {
                    (State::Ok, Some("resolved"))
                } else {
                    (current, None)
                }
            }
        };
        // A subject at rest has no entry: `state` reads a missing one as Ok, and a subject that went away
        // (a VM stopped, an agent deleted) must not leave an Ok entry behind.
        if next == State::Ok {
            self.states.remove(&key);
        } else {
            self.states.insert(key, next);
        }
        what.map(|what| Transition {
            rule: rule.id.clone(),
            subject: subject.to_string(),
            what: what.to_string(),
            value,
            at_ms: now_ms,
        })
    }

    /// The state of a rule's subject.
    pub fn state(&self, rule: &str, subject: &str) -> State {
        self.states
            .get(&format!("{rule}\u{1f}{subject}"))
            .copied()
            .unwrap_or(State::Ok)
    }

    /// Every firing (or clearing) subject of a rule with its since time.
    pub fn firing(&self, rule: &str) -> Vec<(String, u64)> {
        let prefix = format!("{rule}\u{1f}");
        let mut out: Vec<(String, u64)> = self
            .states
            .iter()
            .filter_map(|(key, state)| {
                let subject = key.strip_prefix(&prefix)?;
                match *state {
                    State::Firing { since_ms } | State::Clearing { since_ms, .. } => {
                        Some((subject.to_string(), since_ms))
                    }
                    _ => None,
                }
            })
            .collect();
        out.sort();
        out
    }
}

/// The starter rules: first-token p95 over 2 s for 10 min (critical), any volume over 90 % for 5 min (critical),
/// an agent over 90 % of its window for 15 min, wake p95 over 1 s for 15 min, daily spend over $60, a node not
/// reporting for 60 s (critical). Ids `starter-ttft`, `starter-disk`, `starter-context`, `starter-wake`,
/// `starter-spend`, `starter-node`.
pub fn starter_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: "starter-ttft".into(),
            name: "First token p95 over 2 s".into(),
            metric: "llm.ttft_ms".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Gt,
            threshold: 2000.0,
            for_ms: 600_000,
            severity: Severity::Critical,
            on: true,
            notify: Notify::default(),
            starter: true,
        },
        Rule {
            id: "starter-disk".into(),
            name: "Volume over 90 % used".into(),
            metric: "volume.used_pct".into(),
            agg: AggFn::Max,
            scope: "every:volume".into(),
            cmp: Cmp::Gt,
            threshold: 90.0,
            for_ms: 300_000,
            severity: Severity::Critical,
            on: true,
            notify: Notify::default(),
            starter: true,
        },
        Rule {
            id: "starter-context".into(),
            name: "Agent window over 90 % full".into(),
            metric: "ctx.fill_pct".into(),
            agg: AggFn::Last,
            scope: "every:agent".into(),
            cmp: Cmp::Gt,
            threshold: 90.0,
            for_ms: 900_000,
            severity: Severity::Warning,
            on: true,
            notify: Notify::default(),
            starter: true,
        },
        Rule {
            id: "starter-wake".into(),
            name: "Wake p95 over 1 s".into(),
            metric: "life.wake_ms".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Gt,
            threshold: 1000.0,
            for_ms: 900_000,
            severity: Severity::Warning,
            on: true,
            notify: Notify::default(),
            starter: true,
        },
        Rule {
            id: "starter-spend".into(),
            name: "Daily spend over $60".into(),
            metric: "llm.spend_usd".into(),
            agg: AggFn::Sum,
            scope: "global".into(),
            cmp: Cmp::Gt,
            threshold: 60.0,
            for_ms: 0,
            severity: Severity::Warning,
            on: true,
            notify: Notify::default(),
            starter: true,
        },
        Rule {
            id: "starter-node".into(),
            name: "Node not reporting for 60 s".into(),
            metric: "link.beat_lag_ms".into(),
            agg: AggFn::Max,
            scope: "every:node".into(),
            cmp: Cmp::Gt,
            threshold: 60_000.0,
            for_ms: 60_000,
            severity: Severity::Critical,
            on: true,
            notify: Notify::default(),
            starter: true,
        },
    ]
}

/// A service-level objective: the share of 5-minute windows in the month in which `metric` (reduced by `agg`)
/// met the condition must be at least `goal_pct`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slo {
    pub id: String,
    pub name: String,
    pub metric: String,
    pub agg: AggFn,
    pub scope: String,
    /// The objective holds when `cmp.holds(value, threshold)`.
    pub cmp: Cmp,
    pub threshold: f64,
    pub goal_pct: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SloStatus {
    /// Share of windows that met the objective, in percent.
    pub met_pct: f64,
    /// Share of the error budget left: 1.0 untouched, 0.0 spent, negative over.
    pub budget_left: f64,
    /// Windows counted.
    pub windows: usize,
}

/// The status of a service level from its 5-minute window values (in the month so far).
pub fn slo_status(slo: &Slo, windows: &[f64]) -> SloStatus {
    if windows.is_empty() {
        return SloStatus {
            met_pct: 100.0,
            budget_left: 1.0,
            windows: 0,
        };
    }
    let holding = windows
        .iter()
        .filter(|value| slo.cmp.holds(**value, slo.threshold))
        .count();
    let met_pct = holding as f64 / windows.len() as f64 * 100.0;
    let allowed = (100.0 - slo.goal_pct) / 100.0;
    let bad = 1.0 - met_pct / 100.0;
    let budget_left = if allowed <= 0.0 {
        if bad > 0.0 { -1.0 } else { 1.0 }
    } else {
        1.0 - bad / allowed
    };
    SloStatus {
        met_pct,
        budget_left,
        windows: windows.len(),
    }
}

/// The starter service levels: wake p95 < 1 s 99 %, first token p95 < 3 s 98 %, hub API p95 < 200 ms 99.9 %,
/// heartbeat lag < 30 s 99.5 %.
pub fn starter_slos() -> Vec<Slo> {
    vec![
        Slo {
            id: "slo-wake".into(),
            name: "Wake p95 < 1 s".into(),
            metric: "life.wake_ms".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Lt,
            threshold: 1000.0,
            goal_pct: 99.0,
        },
        Slo {
            id: "slo-ttft".into(),
            name: "First token p95 < 3 s".into(),
            metric: "llm.ttft_ms".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Lt,
            threshold: 3000.0,
            goal_pct: 98.0,
        },
        Slo {
            id: "slo-api".into(),
            name: "Hub API p95 < 200 ms".into(),
            metric: "hub.http_ms".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Lt,
            threshold: 200.0,
            goal_pct: 99.9,
        },
        Slo {
            id: "slo-beat".into(),
            name: "Heartbeat lag < 30 s".into(),
            metric: "link.beat_lag_ms".into(),
            agg: AggFn::Max,
            scope: "global".into(),
            cmp: Cmp::Lt,
            threshold: 30_000.0,
            goal_pct: 99.5,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{Cmp, Notify, Rule, Severity, Slo, SloStatus, State, Tracker};
    use crate::query::AggFn;

    fn rule(for_ms: u64) -> Rule {
        Rule {
            id: "test-rule".into(),
            name: "Test rule".into(),
            metric: "test.metric".into(),
            agg: AggFn::Last,
            scope: "global".into(),
            cmp: Cmp::Gt,
            threshold: 90.0,
            for_ms,
            severity: Severity::Warning,
            on: true,
            notify: Notify::default(),
            starter: false,
        }
    }

    #[test]
    fn a_rule_fires_only_after_holding_for_its_duration() {
        let rule = rule(1_000);
        let mut tracker = Tracker::default();
        assert!(tracker.observe(&rule, "global", 95.0, 0).is_none());
        assert!(matches!(
            tracker.state("test-rule", "global"),
            State::Pending { since_ms: 0 }
        ));
        assert!(tracker.observe(&rule, "global", 95.0, 999).is_none());
        let fired = tracker
            .observe(&rule, "global", 95.0, 1_000)
            .expect("the rule fires once it has held for its duration");
        assert_eq!(fired.what, "fired");
        assert_eq!(fired.rule, "test-rule");
        assert_eq!(fired.subject, "global");
        assert_eq!(fired.at_ms, 1_000);
        assert_eq!(
            tracker.firing("test-rule"),
            vec![("global".to_string(), 1_000)]
        );
    }

    #[test]
    fn a_subject_back_at_rest_leaves_no_state_behind() {
        let rule = rule(0);
        let mut tracker = Tracker::default();
        tracker.observe(&rule, "vm-gone", 95.0, 0);
        assert_eq!(tracker.states.len(), 1);
        tracker.observe(&rule, "vm-gone", 10.0, 1);
        assert!(tracker.states.is_empty());
        for subject in ["a", "b", "c"] {
            tracker.observe(&rule, subject, 10.0, 2);
        }
        assert!(tracker.states.is_empty());
        assert_eq!(tracker.state("test-rule", "a"), State::Ok);
    }

    #[test]
    fn a_rule_resolves_only_after_staying_clear_and_a_blip_keeps_it_firing() {
        let rule = rule(1_000);
        let mut tracker = Tracker::default();
        tracker.observe(&rule, "global", 95.0, 0);
        tracker.observe(&rule, "global", 95.0, 1_000);
        assert!(tracker.observe(&rule, "global", 50.0, 2_000).is_none());
        assert!(matches!(
            tracker.state("test-rule", "global"),
            State::Clearing {
                since_ms: 1_000,
                clear_ms: 2_000
            }
        ));
        assert!(tracker.observe(&rule, "global", 95.0, 2_500).is_none());
        assert!(matches!(
            tracker.state("test-rule", "global"),
            State::Firing { since_ms: 1_000 }
        ));
        assert!(tracker.observe(&rule, "global", 50.0, 3_000).is_none());
        assert!(tracker.observe(&rule, "global", 50.0, 3_500).is_none());
        let resolved = tracker
            .observe(&rule, "global", 50.0, 4_000)
            .expect("the rule resolves after staying clear for its duration");
        assert_eq!(resolved.what, "resolved");
        assert_eq!(resolved.at_ms, 4_000);
        assert!(tracker.firing("test-rule").is_empty());
    }

    #[test]
    fn a_rule_with_no_duration_fires_and_resolves_at_once() {
        let rule = rule(0);
        let mut tracker = Tracker::default();
        let fired = tracker
            .observe(&rule, "global", 95.0, 10)
            .expect("a rule with no duration fires on the first reading");
        assert_eq!(fired.what, "fired");
        let resolved = tracker
            .observe(&rule, "global", 50.0, 20)
            .expect("a rule with no duration resolves on the first clear reading");
        assert_eq!(resolved.what, "resolved");
        assert_eq!(resolved.at_ms, 20);
    }

    #[test]
    fn an_slo_budget_goes_negative_when_the_goal_is_missed() {
        let slo = Slo {
            id: "slo-test".into(),
            name: "Test level".into(),
            metric: "test.metric".into(),
            agg: AggFn::P95,
            scope: "global".into(),
            cmp: Cmp::Lt,
            threshold: 100.0,
            goal_pct: 99.0,
        };
        let mut windows = vec![50.0; 97];
        windows.extend([200.0; 3]);
        let status: SloStatus = super::slo_status(&slo, &windows);
        assert_eq!(status.windows, 100);
        assert!((status.met_pct - 97.0).abs() < 1e-9);
        assert!(status.budget_left < 0.0);
        assert!((status.budget_left - -2.0).abs() < 1e-9);
    }
}
