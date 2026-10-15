//! Alert rules and service levels.
//!
//! A rule is one metric (reduced by an [`AggFn`]) in one scope, compared with a threshold for a duration. The hub
//! checks a rule each time the current window of a series it watches updates; there is no polling loop. A
//! subject (the scope, or each entity for an "every …" scope) moves ok → pending → firing when the condition holds
//! for `for_ms`, and firing → resolved → ok when it stays clear for `for_ms` (so it cannot flap).

use crate::query::AggFn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Critical,
}

/// Where an alert is told. Nothing leaves the plaza.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub notify: Notify,
    /// One of the starter rules (they can be switched off, not deleted).
    #[serde(default)]
    pub starter: bool,
}

fn yes() -> bool {
    true
}

/// A subject's state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
        let _ = (rule, subject, value, now_ms);
        todo!(
            "key = rule id, U+001F, subject; implement the ok/pending/firing/clearing machine with for_ms both ways"
        )
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
        let _ = rule;
        todo!()
    }
}

/// The starter rules: first-token p95 over 2 s for 10 min (critical), any volume over 90 % for 5 min (critical),
/// an agent over 90 % of its window for 15 min, wake p95 over 1 s for 15 min, daily spend over $60, a node not
/// reporting for 60 s (critical). Ids `starter-ttft`, `starter-disk`, `starter-context`, `starter-wake`,
/// `starter-spend`, `starter-node`.
pub fn starter_rules() -> Vec<Rule> {
    todo!()
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
    let _ = (slo, windows);
    todo!(
        "met = count holding / len * 100; allowed bad = (100-goal)/100; budget_left = 1 - bad_share/allowed (1.0 when no windows)"
    )
}

/// The starter service levels: wake p95 < 1 s 99 %, first token p95 < 3 s 98 %, hub API p95 < 200 ms 99.9 %,
/// heartbeat lag < 30 s 99.5 %.
pub fn starter_slos() -> Vec<Slo> {
    todo!()
}
