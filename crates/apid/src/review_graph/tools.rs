//! The agents' review and Weft tools.
//!
//! Review (`review_*`) works on the step the calling agent was given; `recipe_propose` is the orchestrator's.
//! Weft (`weft_*`): a worker reads and describes its own lane; anyone reads their project's
//! history within the path rules. No tool lands, releases, exports, moves a ref or undoes another's op.

use serde_json::{Value, json};

use super::{Finding, Score, Touch};
use crate::agents::Agent;
use crate::state::App;

pub const TOOLS: &[&str] = &[
    "review_tree",
    "review_score",
    "review_verdict",
    "review_spawn",
    "recipe_propose",
    "weft_status",
    "weft_diff",
    "weft_describe",
    "weft_log",
    "weft_show",
    "weft_blame",
    "weft_map",
    "weft_change_tree",
    "weft_touches",
    "weft_bisect",
    "weft_release_notes",
];

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    let score = json!({"type": "object", "properties": {
        "symbol": {"type": "string", "description": "path::definition (as review_tree names it), or a path"},
        "axis": {"type": "string"}, "level": {"type": "integer", "minimum": -5, "maximum": 5},
        "confidence": {"type": "number", "minimum": 0, "maximum": 1}, "basis": {"type": "string", "enum": ["measured", "estimated"]},
        "evidence": {"type": "string"}
    }, "required": ["symbol", "axis", "level", "basis", "evidence"], "additionalProperties": false});
    let finding = json!({"type": "object", "properties": {"symbol": {"type": "string"}, "text": {"type": "string"}, "severity": {"type": "string", "enum": ["info", "minor", "major", "blocker"]}}, "required": ["symbol", "text"], "additionalProperties": false});
    let touch = json!({"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}, "how": {"type": "string"}, "note": {"type": "string"}}, "required": ["from", "to", "how"], "additionalProperties": false});
    let intent = json!({"type": "object", "properties": {"title": {"type": "string"}, "kind": {"type": "string", "enum": ["feature", "fix", "improvement", "refactor", "docs", "test", "research", "chore"]}, "summary": {"type": "string"}, "covers": {"type": "array", "items": {"type": "string"}, "description": "paths, globs, or path::definition"}}, "required": ["title", "kind", "covers"], "additionalProperties": false});
    Some(match name {
        "review_tree" => (
            "The change your review step reads: its definitions by intent, what each touches, who wrote which lines, divergence from other open lanes, findings from earlier rounds, and the raw diff.",
            obj(
                json!({"path": {"type": "string", "description": "only this file's diff"}}),
                &[],
            ),
        ),
        "review_score" => (
            "Score definitions on your step's axes (-5 much worse … 5 much better, with confidence, basis and evidence); add findings anchored to definitions, and how parts touch the rest.",
            obj(
                json!({"scores": {"type": "array", "items": score}, "findings": {"type": "array", "items": finding}, "touches": {"type": "array", "items": touch}}),
                &["scores"],
            ),
        ),
        "review_verdict" => (
            "Pass or fail your review step, with short notes. Signed with your key in Weft.",
            obj(
                json!({"pass": {"type": "boolean"}, "notes": {"type": "string"}}),
                &["pass", "notes"],
            ),
        ),
        "review_spawn" => (
            "Add a sub-review to your step (within its limit): a check that measures something, or a narrower agent review.",
            obj(
                json!({"kind": {"type": "string", "enum": ["check", "agent"]}, "role": {"type": "string"}, "axes": {"type": "array", "items": {"type": "string"}}, "commands": {"type": "array", "items": {"type": "string"}}, "ask": {"type": "string"}}),
                &["kind", "ask"],
            ),
        ),
        "recipe_propose" => (
            "Propose a review recipe or a new version of one (TOML: id, version, axes, nodes, rules); the owner approves it.",
            obj(
                json!({"toml": {"type": "string"}, "why": {"type": "string"}}),
                &["toml", "why"],
            ),
        ),
        "weft_status" => (
            "Your task's lane in Weft: its drafts, rounds, intents and the files it changes.",
            obj(json!({}), &[]),
        ),
        "weft_diff" => (
            "The diff of your lane's latest draft against its base.",
            obj(json!({"path": {"type": "string"}}), &[]),
        ),
        "weft_describe" => (
            "Name what your change does before you finish: one intent per thing, with what each covers. Each intent becomes one commit when the change is exported.",
            obj(
                json!({"intents": {"type": "array", "items": intent, "minItems": 1}}),
                &["intents"],
            ),
        ),
        "weft_log" => (
            "Your project's history: changes matching a query (lane(t51), landed(), touches(path::fn), by(agent:107), since(7d), … combined with & | ~).",
            obj(
                json!({"query": {"type": "string"}, "repo": {"type": "string"}, "limit": {"type": "integer"}}),
                &[],
            ),
        ),
        "weft_show" => (
            "One change: its revisions, intents and sign-off records.",
            obj(json!({"change": {"type": "string"}}), &["change"]),
        ),
        "weft_blame" => (
            "Who wrote each line of a file (or of one definition), on main.",
            obj(
                json!({"path": {"type": "string"}, "symbol": {"type": "string"}, "repo": {"type": "string"}}),
                &["path"],
            ),
        ),
        "weft_map" => (
            "Your project's map: its components, or where something lives (\"where is the approvals screen?\").",
            obj(
                json!({"question": {"type": "string"}, "repo": {"type": "string"}}),
                &[],
            ),
        ),
        "weft_change_tree" => (
            "The change tree of your lane's latest draft: its files and definitions, who wrote them, what each touches and its intents.",
            obj(json!({}), &[]),
        ),
        "weft_touches" => (
            "The changes in your project's history that touch a path, or one definition in it (the same query as weft_log's touches(path::fn)).",
            obj(
                json!({"path": {"type": "string"}, "symbol": {"type": "string"}, "repo": {"type": "string"}, "limit": {"type": "integer"}}),
                &["path"],
            ),
        ),
        "weft_bisect" => (
            "Find the landed change that broke a check: the landed revisions to halve, and the command that does it in your VM.",
            obj(
                json!({"check": {"type": "string"}, "repo": {"type": "string"}}),
                &["check"],
            ),
        ),
        "weft_release_notes" => (
            "Draft the notes of the next release from the intents landed since the last one (never signs).",
            obj(
                json!({"repo": {"type": "string"}, "from": {"type": "string"}}),
                &[],
            ),
        ),
        _ => return None,
    })
}

fn arr<T: serde::de::DeserializeOwned>(input: &Value, key: &str) -> Result<Vec<T>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(vec![]),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("{key}: {e}")),
    }
}

fn text(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// The agent's project repo (or the one named, if it is in the agent's project).
fn repo_of(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let project = crate::projects::project_of(app, agent.cid).ok_or("you are not on a project")?;
    let own = crate::weft_hub::ensure_repo(app, &project)?;
    match input.get("repo").and_then(Value::as_str) {
        Some(r) if r != own => Err(format!("{r} is not your project's repo ({own})")),
        _ => Ok(own),
    }
}

/// The lane of the work the agent is doing: its item's own (a task's, an editor session's), else its task's.
fn own_lane(app: &App, agent: &Agent) -> Result<String, String> {
    let item = app
        .work
        .current(agent.cid)
        .ok_or("you are not working on a task")?;
    if let Some(lane) = item.origin.lane {
        return Ok(lane.change);
    }
    let task = item.origin.task.ok_or("your current work is not a task")?;
    app.weft
        .lane_of(&task)
        .ok_or_else(|| format!("task {task} has no lane in Weft"))
}

pub fn run(
    app: &App,
    agent: &Agent,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    let me = format!("agent:{}", agent.cid);
    let weft = || app.weft.client(&me);
    let enc = weft::server::percent_encode;
    match name {
        "review_tree" => {
            let (run_id, node) = app
                .reviews
                .step_for(agent.cid, work)
                .ok_or("you have no review step")?;
            let run = app.reviews.run(&run_id).ok_or("no such review")?;
            let n = run.node(&node).ok_or("no such step")?.clone();
            let mut diff = weft()?.get(&format!(
                "/revisions/{}/diff{}",
                run.revision,
                input
                    .get("path")
                    .and_then(Value::as_str)
                    .map(|p| format!("?path={}", enc(p)))
                    .unwrap_or_default()
            ))?;
            // The unified text is what an agent reads; the hunks would say it twice.
            let mut unified = String::new();
            for f in diff["files"].as_array().into_iter().flatten() {
                unified.push_str(f["unified"].as_str().unwrap_or(""));
            }
            if unified.len() > 60_000 {
                unified.truncate(60_000);
                unified.push_str("\n… (cut: ask for one path)");
            }
            diff = json!(unified);
            let earlier = super::carried_findings(app, &run);
            let tree = app.reviews.tree(&run);
            Ok(text(&json!({
                "run": run.id, "round": run.round, "step": node, "axes": n.def.axes, "ask": n.def.ask, "recipe": run.recipe,
                "intents": tree["intents"], "unsorted": tree["unsorted"], "files": tree["files"], "divergence": tree["divergence"],
                "earlier_findings": earlier, "diff": diff,
            })))
        }
        "review_score" => {
            let scores: Vec<Score> = arr(input, "scores")?;
            let findings: Vec<Finding> = arr(input, "findings")?;
            let touches: Vec<Touch> = arr(input, "touches")?;
            super::report(app, agent.cid, work, scores, findings, touches)
        }
        "review_verdict" => {
            let pass = input
                .get("pass")
                .and_then(Value::as_bool)
                .ok_or("missing boolean `pass`")?;
            let notes = input.get("notes").and_then(Value::as_str).unwrap_or("");
            super::verdict(app, agent.cid, work, pass, notes)
        }
        "review_spawn" => {
            let kind = input.get("kind").and_then(Value::as_str).unwrap_or("agent");
            super::spawn(
                app,
                agent.cid,
                work,
                kind,
                input
                    .get("role")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                arr(input, "axes")?,
                arr(input, "commands")?,
                input.get("ask").and_then(Value::as_str).unwrap_or(""),
            )
        }
        "recipe_propose" => {
            let toml = input
                .get("toml")
                .and_then(Value::as_str)
                .ok_or("missing `toml`")?;
            let r = super::recipe::Recipe::parse(toml)?;
            let p = app.reviews.propose(
                agent.cid,
                r,
                input.get("why").and_then(Value::as_str).unwrap_or(""),
            )?;
            app.bus.publish(
                "recipe_proposal",
                &json!({ "id": p.id, "recipe": p.recipe.key(), "by": agent.cid }),
            );
            Ok(format!(
                "proposal {} ({}) waits for the owner",
                p.id,
                p.recipe.key()
            ))
        }
        "weft_status" => {
            let lane = own_lane(app, agent)?;
            let c = weft()?.get(&format!("/changes/{lane}"))?;
            let latest = c["drafts"].as_array().and_then(|d| d.last()).cloned();
            let files = match latest.as_ref().and_then(|l| l.as_str()) {
                Some(l) => weft()?.get(&format!("/revisions/{l}"))?["deltas"]
                    .as_array()
                    .map(|d| {
                        d.iter()
                            .map(|x| json!({ "path": x["path"], "deleted": x["after"].is_null() }))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                None => vec![],
            };
            Ok(text(
                &json!({ "change": lane, "state": c["state"], "base": c["base"], "drafts": c["drafts"], "rounds": c["rounds"], "intents": c["intents"], "files": files }),
            ))
        }
        "weft_diff" => {
            let lane = own_lane(app, agent)?;
            let c = weft()?.get(&format!("/changes/{lane}"))?;
            let latest = c["drafts"]
                .as_array()
                .and_then(|d| d.last())
                .and_then(|d| d.as_str())
                .ok_or("your lane has no draft yet")?
                .to_string();
            let d = weft()?.get(&format!(
                "/revisions/{latest}/diff{}",
                input
                    .get("path")
                    .and_then(Value::as_str)
                    .map(|p| format!("?path={}", enc(p)))
                    .unwrap_or_default()
            ))?;
            let mut out = String::new();
            for f in d["files"].as_array().into_iter().flatten() {
                out.push_str(
                    f["unified"]
                        .as_str()
                        .unwrap_or(&format!("binary {}\n", f["path"].as_str().unwrap_or(""))),
                );
            }
            Ok(if out.is_empty() {
                "no changes from the base".into()
            } else {
                out
            })
        }
        "weft_describe" => {
            let lane = own_lane(app, agent)?;
            let intents = input.get("intents").cloned().ok_or("missing `intents`")?;
            let c = weft()?.get(&format!("/changes/{lane}"))?;
            let n = c["rounds"].as_array().map(|r| r.len()).unwrap_or(0) + 1;
            crate::weft_hub::sign_as_agent(
                app,
                agent.cid,
                "intent.set",
                json!({ "change": lane, "n": n, "intents": intents }),
            )?;
            Ok(format!("{lane}: intents set for round {n}"))
        }
        "weft_log" => {
            let repo = repo_of(app, agent, input)?;
            let q = input
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("all()");
            let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(30);
            Ok(text(&weft()?.get(&format!(
                "/repos/{repo}/log?q={}&limit={limit}",
                enc(q)
            ))?))
        }
        "weft_show" => {
            let change = input
                .get("change")
                .and_then(Value::as_str)
                .ok_or("missing `change`")?;
            Ok(text(&weft()?.get(&format!("/changes/{}", enc(change)))?))
        }
        "weft_blame" => {
            let repo = repo_of(app, agent, input)?;
            let path = input
                .get("path")
                .and_then(Value::as_str)
                .ok_or("missing `path`")?;
            let mut url = format!("/repos/{repo}/blame?path={}", enc(path));
            if let Some(s) = input.get("symbol").and_then(Value::as_str) {
                url.push_str(&format!("&symbol={}", enc(s)));
            }
            Ok(text(&weft()?.get(&url)?))
        }
        "weft_map" => {
            let repo = repo_of(app, agent, input)?;
            match input.get("question").and_then(Value::as_str) {
                Some(q) => Ok(text(
                    &weft()?.get(&format!("/repos/{repo}/map/where?q={}", enc(q)))?,
                )),
                None => Ok(text(&weft()?.get(&format!("/repos/{repo}/map"))?)),
            }
        }
        "weft_change_tree" => {
            let lane = own_lane(app, agent)?;
            let c = weft()?.get(&format!("/changes/{lane}"))?;
            let latest = c["drafts"]
                .as_array()
                .and_then(|d| d.last())
                .and_then(|d| d.as_str())
                .ok_or("your lane has no draft yet")?
                .to_string();
            Ok(text(&weft()?.get(&format!("/revisions/{latest}/tree"))?))
        }
        "weft_touches" => {
            let repo = repo_of(app, agent, input)?;
            let path = input
                .get("path")
                .and_then(Value::as_str)
                .ok_or("missing `path`")?;
            let query = touches_query(path, input.get("symbol").and_then(Value::as_str))?;
            let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(30);
            Ok(text(&weft()?.get(&format!(
                "/repos/{repo}/log?q={}&limit={limit}",
                enc(&query)
            ))?))
        }
        "weft_bisect" => {
            let repo = repo_of(app, agent, input)?;
            let check = input
                .get("check")
                .and_then(Value::as_str)
                .ok_or("missing `check`")?;
            let refs = weft()?.get(&format!("/repos/{repo}/refs/main"))?;
            Ok(text(
                &json!({ "run_in_your_vm": format!("weft bisect --repo {repo} --check {}", shell_quote(check)), "landed": refs["revisions"] }),
            ))
        }
        "weft_release_notes" => {
            let repo = repo_of(app, agent, input)?;
            let from = input
                .get("from")
                .and_then(Value::as_str)
                .map(|f| format!("?from={}", enc(f)))
                .unwrap_or_default();
            Ok(text(
                &weft()?.get(&format!("/repos/{repo}/releases/notes{from}"))?,
            ))
        }
        other => Err(format!("no tool {other}")),
    }
}

/// The log query of `weft_touches`: `touches(path)`, or `touches(path::symbol)`. A name that carries the
/// query's own punctuation or a space is refused, so it cannot change the query.
fn touches_query(path: &str, symbol: Option<&str>) -> Result<String, String> {
    // Whitespace and the query's operators, and `:` (the path and the symbol are joined by `::`).
    let plain =
        |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || "()&|~:".contains(c));
    if !plain(path) {
        return Err(format!("invalid input: `path` is not a path: {path:?}"));
    }
    match symbol {
        None => Ok(format!("touches({path})")),
        Some(symbol) if plain(symbol) => Ok(format!("touches({path}::{symbol})")),
        Some(symbol) => Err(format!("invalid input: `symbol` is not a name: {symbol:?}")),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touches_names_a_path_or_a_definition_and_refuses_what_would_change_the_query() {
        assert_eq!(
            touches_query("src/a.rs", None),
            Ok("touches(src/a.rs)".into())
        );
        assert_eq!(
            touches_query("src/a.rs", Some("parse")),
            Ok("touches(src/a.rs::parse)".into())
        );
        assert!(touches_query("src/a.rs) | all()", None).is_err());
        assert!(touches_query("", None).is_err());
        assert!(touches_query("src/a.rs", Some("x & y")).is_err());
    }
}
