//! Recipe files (PLAN 4.4, 21.1): a recipe is a graph of steps, kept as a TOML file. A built-in recipe ships in the hub
//! (`builtin/*.toml`); a project's own copy lives in its Weft repo at `.plaza/recipes/<id>.toml` and overrides the
//! built-in of the same id; the owner's new versions are saved in the hub (`runs.rs`).
//!
//! ```toml
//! id = "change"
//! version = 1
//! inputs = ["task"]
//! outputs = ["change"]
//!
//! [[steps]]
//! id = "build"
//! role = "builder"
//! model = "any"              # any | differ | a model (provider/name) | a list of models | eval
//! fan_out = 1                # a count, or per_file | per_target | per_finding
//! gate = ["passed"]          # every condition must hold (default: passed)
//!
//! [[edges]]
//! from = "build"
//! to = "review"
//! when = "passed"            # passed | failed | findings>0 | findings == 0 | score >= 3 | ...
//! max = 2                    # a loop edge: taken at most max times, and it rewinds its target
//! ```
//!
//! A step is named by its `id`; the seat it fills (`seat`, default its id) is what a task's sessions are addressed to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::graph::Cond;

/// Where a recipe applies: the built-ins, a project's own, or every project's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    BuiltIn,
    Project,
    Every,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// Which models a step's session may use: a name (`any`, `differ`, `eval`, or one model) or a list to choose from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Model {
    Name(String),
    List(Vec<String>),
}

impl Default for Model {
    fn default() -> Self {
        Model::Name("any".into())
    }
}

/// How many items a step runs: a fixed count, or one per file, target or finding of the run's input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fan {
    Count(u32),
    Named(String),
}

impl Default for Fan {
    fn default() -> Self {
        Fan::Count(1)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    /// The seat the step's session fills (default: the step's id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// The role of the session that runs the step. A step without one is run by its engine (`engine`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Model,
    /// Who runs a step that is not a session of the work loop: `review_graph`, `base`, `curator`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub fan_out: Fan,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate: Vec<String>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub budget_share: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

impl Step {
    /// The seat the step's session fills.
    pub fn seat_name(&self) -> &str {
        self.seat.as_deref().unwrap_or(&self.id)
    }

    /// The gate's conditions: the step's own, or the default one (its verdict passed).
    pub fn gate_conditions(&self) -> Vec<String> {
        if self.gate.is_empty() {
            vec!["passed".to_string()]
        } else {
            self.gate.clone()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub when: String,
    /// A loop: taken at most this many times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

/// The file a project keeps its copy of a recipe in, in its Weft repo at main.
pub fn project_file(id: &str) -> String {
    format!(".plaza/recipes/{id}.toml")
}

impl Recipe {
    pub fn parse(text: &str) -> Result<Recipe, String> {
        let r: Recipe = toml::from_str(text).map_err(|e| format!("recipe: {e}"))?;
        r.check()?;
        Ok(r)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// `id@version`, as runs and seats name the recipe they run.
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// Whether a step has an edge into it that is not a loop (it starts only after that edge is taken).
    pub fn has_forward_edge_into(&self, step: &str) -> bool {
        self.edges.iter().any(|e| e.to == step && e.max.is_none())
    }

    /// A recipe must make sense before a run starts on it: unique ids, edges between steps that exist, conditions that
    /// parse, loops with a bound, forward edges that make no cycle, and a step to start with.
    pub fn check(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.version == 0 {
            return Err("a recipe needs an id and a version".into());
        }
        if self.steps.is_empty() {
            return Err(format!("recipe {} has no steps", self.id));
        }
        let mut seats: BTreeMap<&str, &str> = BTreeMap::new();
        for (i, step) in self.steps.iter().enumerate() {
            if step.id.trim().is_empty() {
                return Err("a step needs an id".into());
            }
            if self.steps[..i].iter().any(|s| s.id == step.id) {
                return Err(format!("step {} appears twice", step.id));
            }
            if let Some(role) = &step.role {
                if role.trim().is_empty() {
                    return Err(format!("step {} has an empty role", step.id));
                }
                match &step.model {
                    Model::Name(m) if m == "any" || m == "differ" => {}
                    _ => {
                        return Err(format!(
                            "step {}: a session's model is any or differ for now (pinned models come with phase 4.2)",
                            step.id
                        ));
                    }
                }
                let seat = step.seat_name();
                if let Some(other) = seats.insert(seat, &step.id) {
                    return Err(format!(
                        "steps {other} and {} fill the same seat {seat}",
                        step.id
                    ));
                }
            } else if step.engine.is_none() {
                return Err(format!("step {} needs a role or an engine", step.id));
            }
            if let Fan::Named(named) = &step.fan_out
                && !matches!(named.as_str(), "per_file" | "per_target" | "per_finding")
            {
                return Err(format!(
                    "step {}: fan_out {named} is not a fan-out",
                    step.id
                ));
            }
            if let Model::List(list) = &step.model
                && list.is_empty()
            {
                return Err(format!("step {}: a model list is empty", step.id));
            }
            if !(step.budget_share > 0.0 && step.budget_share <= 1.0) {
                return Err(format!("step {}: budget_share is between 0 and 1", step.id));
            }
            for condition in step.gate_conditions() {
                Cond::parse(&condition).map_err(|e| format!("step {} gate: {e}", step.id))?;
            }
        }
        for edge in &self.edges {
            for end in [&edge.from, &edge.to] {
                if self.step(end).is_none() {
                    return Err(format!("an edge names {end}, which is not a step"));
                }
            }
            Cond::parse(&edge.when)
                .map_err(|e| format!("edge {} -> {}: {e}", edge.from, edge.to))?;
            if edge.max == Some(0) {
                return Err(format!(
                    "the loop {} -> {} has no bound",
                    edge.from, edge.to
                ));
            }
        }
        if self.steps.iter().all(|s| self.has_forward_edge_into(&s.id)) {
            return Err("a recipe needs a step that starts it".into());
        }
        // The forward edges (every edge but a loop) make no cycle: every step can be placed after the ones it follows.
        let mut placed: Vec<&str> = Vec::new();
        while placed.len() < self.steps.len() {
            let before = placed.len();
            for s in &self.steps {
                if placed.contains(&s.id.as_str()) {
                    continue;
                }
                let follows = self
                    .edges
                    .iter()
                    .filter(|e| e.to == s.id && e.max.is_none())
                    .all(|e| placed.contains(&e.from.as_str()));
                if follows {
                    placed.push(&s.id);
                }
            }
            if placed.len() == before {
                return Err("the recipe's edges make a cycle without a loop bound".into());
            }
        }
        Ok(())
    }
}

/// The built-in recipes, in the hub.
pub fn builtin() -> Vec<Recipe> {
    [
        include_str!("builtin/change.toml"),
        include_str!("builtin/review.toml"),
        include_str!("builtin/ci.toml"),
        include_str!("builtin/base.toml"),
        include_str!("builtin/distill.toml"),
        include_str!("builtin/release.toml"),
    ]
    .iter()
    .map(|t| Recipe::parse(t).expect("a built-in recipe parses"))
    .collect()
}

/// The built-in recipe named `id`.
pub fn builtin_named(id: &str) -> Option<Recipe> {
    builtin().into_iter().find(|r| r.id == id)
}

/// A project's copy of recipe `id`, from its repo's files at main: `listing` is Weft's file listing of the revision and
/// `fetch` reads one file. `Ok(None)` when the project keeps no copy.
pub fn in_listing(
    id: &str,
    listing: &serde_json::Value,
    fetch: impl FnOnce(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<Recipe>, String> {
    let path = project_file(id);
    let present = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|file| file["path"].as_str() == Some(path.as_str()));
    if !present {
        return Ok(None);
    }
    let text = String::from_utf8(fetch(&path)?).map_err(|_| format!("{path} is not UTF-8"))?;
    let recipe = Recipe::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    if recipe.id != id {
        return Err(format!("{path} is recipe {}, not {id}", recipe.id));
    }
    if recipe.scope == Scope::BuiltIn {
        return Err(format!(
            "{path}: a project's recipe is project or every scope"
        ));
    }
    Ok(Some(recipe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CHANGE: &str = r#"
id = "change"
version = 1
[[steps]]
id = "build"
role = "builder"
[[steps]]
id = "review"
seat = "reviewer"
role = "reviewer"
model = "differ"
[[edges]]
from = "build"
to = "review"
when = "passed"
[[edges]]
from = "review"
to = "build"
when = "failed"
max = 2
"#;

    #[test]
    fn the_built_in_recipes_check_and_round_trip() {
        let recipes = builtin();
        assert_eq!(recipes.len(), 6);
        for r in recipes {
            r.check().unwrap();
            assert_eq!(Recipe::parse(&r.to_toml()).unwrap(), r, "{}", r.id);
        }
        let change = builtin_named("change").unwrap();
        assert_eq!(change.steps[1].seat_name(), "reviewer");
        assert_eq!(change.edges[1].max, Some(2));
    }

    #[test]
    fn a_bad_graph_is_refused_and_says_why() {
        let mut r = Recipe::parse(CHANGE).unwrap();
        r.edges.push(Edge {
            from: "review".into(),
            to: "nowhere".into(),
            when: "passed".into(),
            max: None,
        });
        assert!(r.check().unwrap_err().contains("nowhere"));

        // Without the loop bound, the two edges are a cycle, and no step starts the recipe.
        let mut r = Recipe::parse(CHANGE).unwrap();
        r.edges[1].max = None;
        assert!(r.check().unwrap_err().contains("starts it"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.edges[1].max = Some(0);
        assert!(r.check().unwrap_err().contains("no bound"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.steps[1].model = Model::Name("provider/pinned".into());
        assert!(r.check().unwrap_err().contains("any or differ"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.steps[0].gate = vec!["looks good".into()];
        assert!(r.check().unwrap_err().contains("not a condition"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.steps[0].seat = Some("reviewer".into());
        assert!(r.check().unwrap_err().contains("same seat"));
    }

    #[test]
    fn a_step_can_take_a_fan_out_a_model_list_and_a_gate() {
        let r = Recipe::parse(
            r#"
id = "scan"
version = 2
[[steps]]
id = "audit"
engine = "scanner"
fan_out = "per_file"
gate = ["tests green", "score >= 3"]
[[steps]]
id = "rank"
engine = "scanner"
model = ["a/one", "b/two"]
fan_out = 4
"#,
        )
        .unwrap();
        assert_eq!(r.steps[0].fan_out, Fan::Named("per_file".into()));
        assert_eq!(r.steps[1].fan_out, Fan::Count(4));
        assert_eq!(r.steps[0].gate_conditions().len(), 2);
        assert_eq!(r.key(), "scan@2");
        let bad = Recipe::parse(
            r#"
id = "scan"
version = 1
[[steps]]
id = "audit"
role = "reviewer"
fan_out = "per_colour"
"#,
        );
        assert!(bad.unwrap_err().contains("not a fan-out"));
    }

    #[test]
    fn a_project_copy_overrides_by_id_and_a_broken_one_is_named() {
        let listing = json!({ "files": [{ "path": ".plaza/recipes/change.toml" }] });
        let text = CHANGE.replace("version = 1", "version = 4\nscope = \"project\"");
        let found = in_listing("change", &listing, |path| {
            assert_eq!(path, ".plaza/recipes/change.toml");
            Ok(text.clone().into_bytes())
        })
        .unwrap()
        .unwrap();
        assert_eq!(found.version, 4);
        assert_eq!(found.scope, Scope::Project);

        let none = in_listing("change", &json!({ "files": [] }), |_| unreachable!()).unwrap();
        assert!(none.is_none());

        let broken = in_listing("change", &listing, |_| Ok(b"id = \"change\"".to_vec()));
        assert!(
            broken
                .unwrap_err()
                .starts_with(".plaza/recipes/change.toml")
        );

        let wrong_id = in_listing(
            "ci",
            &json!({ "files": [{ "path": ".plaza/recipes/ci.toml" }] }),
            |_| Ok(text.clone().into_bytes()),
        );
        assert!(wrong_id.unwrap_err().contains("not ci"));
    }
}
