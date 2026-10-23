//! Recipes: saved, versioned review graphs (R1). A recipe names its axes, its nodes (tool, check, agent, gate, owner)
//! and the links between them, the first rules that trim the graph for a kind of change, the gate's rules, and how
//! many rework rounds a task gets before the owner decides (R11).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Recipe {
    pub id: String,
    pub version: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    #[table(child)]
    pub applies: Applies,
    #[serde(default)]
    #[table(list)]
    pub axes: Vec<String>,
    #[serde(default = "two")]
    pub rounds: u32,
    #[serde(default, rename = "first")]
    #[table(child)]
    pub first: Vec<FirstRule>,
    #[serde(default, rename = "node")]
    #[table(child)]
    pub nodes: Vec<NodeDef>,
    #[serde(default, rename = "rule")]
    #[table(child)]
    pub rules: Vec<Rule>,
    /// The slow checks that run on main after a change lands; a failure lands a revert at once.
    #[serde(default)]
    #[table(list)]
    pub post_land: Vec<String>,
}

fn two() -> u32 {
    2
}

/// What a recipe is for: kinds of change (`code`, `docs`, `research`) and task roles.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Applies {
    #[serde(default)]
    #[table(list)]
    pub kinds: Vec<String>,
    #[serde(default)]
    #[table(list)]
    pub roles: Vec<String>,
}

/// A rule applied before the run: for a kind of change, keep only some nodes, or add reviewers.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct FirstRule {
    /// `comment-only`, `test-only`, `docs-only`, or `touches:<glob>,<glob>`.
    pub when: String,
    /// Only these nodes run (the gate always does).
    #[serde(default)]
    #[table(list)]
    pub only: Vec<String>,
    /// More nodes: `role:<role>` adds an agent node of that role on the recipe's axes it names after `:`
    /// (`role:security-reviewer:sec,cmp`); `owner` sends the outcome to the owner.
    #[serde(default)]
    #[table(list)]
    pub add: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct NodeDef {
    pub id: String,
    /// `tool`, `check`, `agent`, `gate`, `owner`.
    pub kind: String,
    /// For a tool: `tree` or `divergence`.
    #[serde(default)]
    pub tool: Option<String>,
    /// For an agent or check node: the role that takes it.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    #[table(list)]
    pub axes: Vec<String>,
    /// For a check node: the commands to run in a fresh checkout.
    #[serde(default)]
    #[table(list)]
    pub commands: Vec<String>,
    /// Nodes that must be done before this one starts.
    #[serde(default)]
    #[table(list)]
    pub after: Vec<String>,
    /// How many sub-reviews an agent node may add.
    #[serde(default)]
    pub sub_reviews: u32,
    /// The node reads the project's invariants and fails a change that breaks one.
    #[serde(default)]
    pub constraints: bool,
    /// What the node is asked, beyond its axes.
    #[serde(default)]
    pub ask: String,
}

/// `when` → `then`. Conditions: `failed` (any node), `<node> failed`, `<axis> <= <level>` (any symbol's lowest
/// score), `measured <axis> <= <level>` (measured scores only). Actions: `rework`, `owner`, `replan`, `fail`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Rule {
    pub when: String,
    pub then: String,
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

    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    /// A recipe's graph must make sense: unique ids, known kinds, links to nodes that exist, one gate, no cycles.
    pub fn check(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.version == 0 {
            return Err("a recipe needs an id and a version".into());
        }
        let ids: Vec<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        for (i, n) in self.nodes.iter().enumerate() {
            if ids[..i].contains(&n.id.as_str()) {
                return Err(format!("node {} appears twice", n.id));
            }
            if !matches!(
                n.kind.as_str(),
                "tool" | "check" | "agent" | "gate" | "owner"
            ) {
                return Err(format!("node {}: no kind {}", n.id, n.kind));
            }
            if matches!(n.kind.as_str(), "agent" | "check") && n.role.is_none() {
                return Err(format!("node {} needs a role", n.id));
            }
            for a in &n.after {
                if !ids.contains(&a.as_str()) {
                    return Err(format!(
                        "node {} runs after {a}, which is not in the recipe",
                        n.id
                    ));
                }
            }
        }
        if self.nodes.iter().filter(|n| n.kind == "gate").count() != 1 {
            return Err("a recipe has exactly one gate".into());
        }
        // No cycles: every node is reachable in an order where its links come first.
        let mut done: Vec<&str> = Vec::new();
        while done.len() < self.nodes.len() {
            let before = done.len();
            for n in &self.nodes {
                if !done.contains(&n.id.as_str())
                    && n.after.iter().all(|a| done.contains(&a.as_str()))
                {
                    done.push(&n.id);
                }
            }
            if done.len() == before {
                return Err("the recipe's links make a cycle".into());
            }
        }
        for r in &self.rules {
            parse_condition(&r.when)?;
            if !matches!(r.then.as_str(), "rework" | "owner" | "replan" | "fail") {
                return Err(format!("rule `{}`: no action {}", r.when, r.then));
            }
        }
        Ok(())
    }
}

/// A gate condition.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    AnyFailed,
    NodeFailed(String),
    Score {
        axis: String,
        at_most: i64,
        measured: bool,
    },
}

pub fn parse_condition(when: &str) -> Result<Condition, String> {
    let w = when.trim();
    if w == "failed" || w == "any failed" {
        return Ok(Condition::AnyFailed);
    }
    if w == "constraint broken" {
        return Ok(Condition::NodeFailed("constraints".into()));
    }
    if let Some(node) = w.strip_suffix(" failed") {
        return Ok(Condition::NodeFailed(node.trim().to_string()));
    }
    let (measured, rest) = match w.strip_prefix("measured ") {
        Some(r) => (true, r),
        None => (false, w),
    };
    if let Some((axis, n)) = rest.split_once("<=") {
        let at_most: i64 = n
            .trim()
            .parse()
            .map_err(|_| format!("`{when}`: not a level"))?;
        return Ok(Condition::Score {
            axis: axis.trim().to_string(),
            at_most,
            measured,
        });
    }
    Err(format!(
        "`{when}` is not a condition (failed, <node> failed, <axis> <= <level>)"
    ))
}

/// The axes' names, as people read them.
pub fn axis_name(axis: &str) -> &str {
    match axis {
        "fea" => "feature",
        "ux" => "UI/UX",
        "val" => "value",
        "con" => "contract",
        "mem" => "memory",
        "cpu" => "compute",
        "prf" => "performance",
        "sec" => "security",
        "cmp" => "compatibility",
        "div" => "divergence",
        "leg" => "legacy",
        "tst" => "tests",
        "cln" => "clean code",
        "dbt" => "tech debt",
        "mnt" => "maintenance",
        "acc" => "accuracy",
        "src" => "sourcing",
        "cov" => "coverage",
        "cost" => "cost",
        "kind" => "change kind",
        other => other,
    }
}

/// The four readings the owner's card and the baseline show, and the axes each sums up.
pub const READINGS: &[(&str, &[&str])] = &[
    ("what it does", &["fea", "ux", "val", "con", "acc", "cov"]),
    ("cost to run", &["mem", "cpu", "prf", "cost"]),
    ("risk", &["sec", "cmp", "div", "leg", "src"]),
    ("code health", &["tst", "cln", "dbt", "mnt"]),
];

const CODE: &str = r#"
id = "code"
version = 3
title = "Code"
axes = ["fea", "ux", "val", "con", "mem", "cpu", "prf", "sec", "cmp", "div", "leg", "tst", "cln", "dbt", "mnt"]
rounds = 2
post_land = ["cargo test --workspace --release"]

[applies]
kinds = ["code"]

[[first]]
when = "comment-only"
only = ["tree", "checks"]

[[first]]
when = "test-only"
only = ["tree", "checks", "quality"]

[[first]]
when = "touches:crates/apid/src/vault/**,crates/apz-trust/**,crates/weft/src/keys.rs"
add = ["role:security-reviewer:sec,cmp,leg", "owner"]

[[node]]
id = "tree"
kind = "tool"
tool = "tree"

[[node]]
id = "checks"
kind = "check"
role = "qa"
axes = ["tst", "prf"]
commands = ["cargo test --workspace"]
after = ["tree"]

[[node]]
id = "function"
kind = "agent"
role = "reviewer"
axes = ["fea", "ux", "val", "con"]
after = ["tree"]
ask = "Does each part do what the task asked, as the acceptance says? Name what each touches."

[[node]]
id = "resources"
kind = "agent"
role = "reviewer"
axes = ["mem", "cpu", "prf"]
after = ["tree"]
sub_reviews = 3
ask = "What does it cost to run? When an estimate matters, add a check that measures it."

[[node]]
id = "risk"
kind = "agent"
role = "security-reviewer"
axes = ["sec", "cmp", "leg"]
after = ["tree"]

[[node]]
id = "divergence"
kind = "tool"
tool = "divergence"
axes = ["div"]
after = ["tree"]

[[node]]
id = "quality"
kind = "agent"
role = "qa"
axes = ["tst", "cln", "dbt", "mnt"]
after = ["tree"]
ask = "Is it tested, and as simple as it can be? Count over-built parts against it."

[[node]]
id = "constraints"
kind = "agent"
role = "reviewer"
after = ["tree"]
constraints = true

[[node]]
id = "gate"
kind = "gate"
after = ["checks", "function", "resources", "risk", "divergence", "quality", "constraints"]

[[rule]]
when = "checks failed"
then = "rework"

[[rule]]
when = "constraint broken"
then = "rework"

[[rule]]
when = "con <= -2"
then = "rework"

[[rule]]
when = "tst <= -3"
then = "rework"

[[rule]]
when = "sec <= -3"
then = "owner"

[[rule]]
when = "cmp <= -2"
then = "owner"

[[rule]]
when = "mem <= -4"
then = "owner"

[[rule]]
when = "cpu <= -4"
then = "owner"

[[rule]]
when = "prf <= -4"
then = "owner"

[[rule]]
when = "div <= -3"
then = "replan"

[[rule]]
when = "failed"
then = "rework"
"#;

const DOCS: &str = r#"
id = "docs"
version = 1
title = "Documentation"
axes = ["fea", "val", "con", "acc", "cln", "mnt", "div"]
rounds = 2

[applies]
kinds = ["docs"]

[[node]]
id = "tree"
kind = "tool"
tool = "tree"

[[node]]
id = "content"
kind = "agent"
role = "reviewer"
axes = ["fea", "val", "con", "acc"]
after = ["tree"]
ask = "Is it right, and does it say what the task asked for, in plain words?"

[[node]]
id = "style"
kind = "agent"
role = "librarian"
axes = ["cln", "mnt"]
after = ["tree"]

[[node]]
id = "divergence"
kind = "tool"
tool = "divergence"
axes = ["div"]
after = ["tree"]

[[node]]
id = "gate"
kind = "gate"
after = ["content", "style", "divergence"]

[[rule]]
when = "acc <= -2"
then = "rework"

[[rule]]
when = "con <= -2"
then = "rework"

[[rule]]
when = "div <= -3"
then = "replan"

[[rule]]
when = "failed"
then = "rework"
"#;

const RESEARCH: &str = r#"
id = "research"
version = 1
title = "Research"
axes = ["acc", "src", "cov", "con", "val", "div", "cost"]
rounds = 2

[applies]
kinds = ["research"]

[[node]]
id = "tree"
kind = "tool"
tool = "tree"

[[node]]
id = "numbers"
kind = "agent"
role = "researcher"
axes = ["acc"]
after = ["tree"]
sub_reviews = 2
ask = "Re-run the numbers the brief gives (from its data files or its sources). Score accuracy as measured when you re-ran them."

[[node]]
id = "sourcing"
kind = "agent"
role = "reviewer"
axes = ["src", "cost"]
after = ["tree"]
ask = "Is every claim sourced, and are the sources fit for it?"

[[node]]
id = "fit"
kind = "agent"
role = "reviewer"
axes = ["con", "cov", "val"]
after = ["tree"]
ask = "Does it answer the brief, all of it, for the people it is for?"

[[node]]
id = "divergence"
kind = "tool"
tool = "divergence"
axes = ["div"]
after = ["tree"]

[[node]]
id = "gate"
kind = "gate"
after = ["numbers", "sourcing", "fit", "divergence"]

[[rule]]
when = "measured acc <= -2"
then = "rework"

[[rule]]
when = "src <= -3"
then = "rework"

[[rule]]
when = "con <= -2"
then = "rework"

[[rule]]
when = "div <= -3"
then = "replan"

[[rule]]
when = "failed"
then = "rework"
"#;

/// The recipes every plaza starts with.
pub fn built_in() -> Vec<Recipe> {
    [CODE, DOCS, RESEARCH]
        .iter()
        .map(|t| Recipe::parse(t).expect("a built-in recipe parses"))
        .collect()
}

/// Which kind of change a change tree is: `code` when any file has a code pack, `docs` for prose in a code
/// project, `research` for prose and data elsewhere.
pub fn kind_of(tree: &Value, project_kind: &str) -> &'static str {
    const PROSE: &[&str] = &["markdown", "text", "json", "yaml", "toml", "html", "css"];
    let code = tree["languages"].as_object().is_some_and(|l| {
        l.keys()
            .any(|k| !PROSE.contains(&k.as_str()) && k != "binary")
    });
    if code {
        "code"
    } else if project_kind == "code" {
        "docs"
    } else {
        "research"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_recipes_check_and_round_trip() {
        for r in built_in() {
            r.check().unwrap();
            assert_eq!(Recipe::parse(&r.to_toml()).unwrap(), r);
        }
        assert_eq!(
            parse_condition("measured acc <= -2").unwrap(),
            Condition::Score {
                axis: "acc".into(),
                at_most: -2,
                measured: true
            }
        );
        assert_eq!(
            parse_condition("checks failed").unwrap(),
            Condition::NodeFailed("checks".into())
        );
    }

    #[test]
    fn a_bad_graph_is_refused() {
        let mut r = built_in().remove(0);
        r.nodes[1].after = vec!["gate".into()];
        assert!(r.check().unwrap_err().contains("cycle"));
        let mut r = built_in().remove(0);
        r.nodes.retain(|n| n.kind != "gate");
        assert!(r.check().is_err());
    }
}
