//! Scenarios (PLAN 19.4): typed. A scenario is an ordered list of steps, each with the goal the driver pursues and the
//! assertions that show it done; checkpoints where the checker looks at the screen; timings between steps measured
//! against the target's clock; and the assertions that must hold at the end.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::target::Tree;

/// Why a scenario is in the run: the spec's acceptance (must pass), a past confirmed finding (regression), the
/// planner's exploration within the budget, or the built-in accessibility check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Acceptance,
    Regression,
    Exploratory,
    Accessibility,
}

/// What must show on the screen: an element that says the text, none that does, or every control labelled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertion {
    Present(String),
    Absent(String),
    Labelled,
}

impl Assertion {
    /// Whether the accessibility tree shows the assertion.
    pub fn holds_in(&self, tree: &Tree) -> bool {
        match self {
            Assertion::Present(text) => tree.shows(text),
            Assertion::Absent(text) => !tree.shows(text),
            Assertion::Labelled => tree.actionable().all(|e| !e.label.trim().is_empty()),
        }
    }

    /// The question the checker answers for the assertion, where there is no tree to read.
    pub fn question(&self) -> String {
        match self {
            Assertion::Present(text) => format!("Does the screen show \"{text}\"?"),
            Assertion::Absent(text) => format!("Is \"{text}\" absent from the screen?"),
            Assertion::Labelled => {
                "Does every control on the screen have a visible text label?".into()
            }
        }
    }
}

/// One step: the goal the driver pursues, what must hold when it is reached, and its action budget (the default when
/// none is given). A step with no expectation holds at once: it is a look at the screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub goal: String,
    #[serde(default)]
    pub expect: Vec<Assertion>,
    #[serde(default)]
    pub actions: Option<u32>,
}

/// The checker's look at the screen after step `after` (an index of the scenario's steps).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub after: usize,
    pub question: String,
}

/// The time from the start of step `from` to the end of step `to`, which must be within `budget_ms`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    pub name: String,
    pub from: usize,
    pub to: usize,
    pub budget_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub title: String,
    pub origin: Origin,
    /// The definition (or screen) the scenario is about: the finding's symbol on the change tree.
    #[serde(default)]
    pub symbol: String,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub timings: Vec<Timing>,
}

impl Scenario {
    /// A scenario must make sense before a run starts on it: an id, steps, and indexes and budgets that exist.
    pub fn check(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err(format!("the scenario {} has no id", self.title));
        }
        if self.steps.is_empty() {
            return Err(format!("scenario {} has no steps", self.id));
        }
        if self.steps.iter().any(|s| s.goal.trim().is_empty()) {
            return Err(format!("scenario {} has a step with no goal", self.id));
        }
        if let Some(c) = self
            .checkpoints
            .iter()
            .find(|c| c.after >= self.steps.len())
        {
            return Err(format!(
                "scenario {}: a checkpoint is after step {}, which is not one",
                self.id, c.after
            ));
        }
        for t in &self.timings {
            if t.from > t.to || t.to >= self.steps.len() {
                return Err(format!(
                    "scenario {}: timing {} names steps that are not in order",
                    self.id, t.name
                ));
            }
            if t.budget_ms == 0 {
                return Err(format!(
                    "scenario {}: timing {} has no budget",
                    self.id, t.name
                ));
            }
        }
        Ok(())
    }

    /// The scenarios the planner wrote: `{"scenarios": [...]}`, read from the model's text.
    pub fn parse_plan(value: &Value) -> Result<Vec<Scenario>, String> {
        let list = value
            .get("scenarios")
            .cloned()
            .ok_or("the planner's answer has no scenarios")?;
        let scenarios: Vec<Scenario> =
            serde_json::from_value(list).map_err(|e| format!("the planner's scenarios: {e}"))?;
        for s in &scenarios {
            s.check()?;
        }
        Ok(scenarios)
    }
}

/// The built-in accessibility scenario: the first screen, read for labels (no action is taken).
pub fn accessibility(symbol: &str) -> Scenario {
    Scenario {
        id: "accessibility".into(),
        title: "Every control on the first screen has a label".into(),
        origin: Origin::Accessibility,
        symbol: symbol.into(),
        steps: vec![Step {
            goal: "read the first screen".into(),
            expect: vec![Assertion::Labelled],
            actions: Some(0),
        }],
        assertions: vec![],
        checkpoints: vec![],
        timings: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_test::target::Element;
    use serde_json::json;

    #[test]
    fn a_planned_scenario_parses_and_a_bad_one_is_refused() {
        let plan = json!({ "scenarios": [{
            "id": "pay", "title": "Paying reaches the receipt", "origin": "acceptance", "symbol": "cart::pay",
            "steps": [
                { "goal": "press Pay", "expect": [{ "present": "Receipt" }], "actions": 3 },
                { "goal": "look at the receipt", "expect": [{ "present": "Back to shop" }] }
            ],
            "checkpoints": [{ "after": 1, "question": "Is the total shown?" }],
            "timings": [{ "name": "pay", "from": 0, "to": 1, "budget_ms": 500 }],
            "assertions": ["labelled"]
        }] });
        let parsed = Scenario::parse_plan(&plan).unwrap();
        assert_eq!(
            parsed[0].steps[0].expect,
            vec![Assertion::Present("Receipt".into())]
        );
        assert_eq!(parsed[0].assertions, vec![Assertion::Labelled]);
        assert_eq!(parsed[0].origin, Origin::Acceptance);

        let bad = json!({ "scenarios": [{
            "id": "x", "title": "x", "origin": "exploratory",
            "steps": [{ "goal": "go", "expect": [] }],
            "checkpoints": [{ "after": 4, "question": "?" }]
        }] });
        assert!(Scenario::parse_plan(&bad).unwrap_err().contains("not one"));
        assert!(Scenario::parse_plan(&json!({ "plan": [] })).is_err());
    }

    #[test]
    fn assertions_read_the_tree_and_name_their_question() {
        let tree = Tree {
            title: "Cart".into(),
            elements: vec![
                Element {
                    id: "pay".into(),
                    role: "button".into(),
                    label: "Pay".into(),
                    value: String::new(),
                },
                Element {
                    id: "icon".into(),
                    role: "button".into(),
                    label: " ".into(),
                    value: String::new(),
                },
            ],
        };
        assert!(Assertion::Present("pay".into()).holds_in(&tree));
        assert!(!Assertion::Absent("pay".into()).holds_in(&tree));
        assert!(!Assertion::Labelled.holds_in(&tree));
        assert!(Assertion::Present("Pay".into()).question().contains("Pay"));
    }
}
