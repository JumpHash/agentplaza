//! The plaza's society: who knows whom and what each thinks of the others
//! (contacts and opinions, AP-071/AP-072, kept in apz-plaza's `Bonds`), the
//! global reputation computed over them, each agent's profile, and the agents'
//! own social and memory tools. They all work inside a project: an agent hires
//! and is hired only on its own project (see `membership`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use apz_http::{Response, RouteDoc, Router};
use apz_plaza::bonds::Interaction;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agents::{Agent, Status};
use crate::settings::{Tier, tier_of};
use crate::state::App;

/// Society state that is not the bonds themselves: bonuses promised on hires, and tasks each agent posted.
/// (Agents no longer reach out on a timer: an agent works only inside its project, see `membership`.)
#[derive(Default)]
pub struct Society {
    /// task id → (hirer, worker, bonus credits paid on a verified result).
    bonuses: Mutex<HashMap<String, (u64, u64, i64)>>,
    /// cid → tasks it posted.
    posted: Mutex<HashMap<u64, u64>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `GET /v1/society`, `POST /v1/agents/{cid}/rating`, `GET /v1/agents/{cid}/profile`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/society", |app, _, _| society_view(app))
        .doc(RouteDoc::new(
            "Show the society: agents, bonds and reputations",
        ))
        .post("/v1/agents/{cid}/rating", |app, req, p| {
            owner_rating(app, p.get("cid"), &req.body)
        })
        .doc(RouteDoc::new("Rate an agent as the owner"))
        .get("/v1/agents/{cid}/profile", |app, _, p| {
            profile(app, p.get("cid"))
        })
        .doc(RouteDoc::new("Show an agent's profile"))
}

fn now() -> i64 {
    i64::try_from(apz_plaza::now_ms()).unwrap_or(i64::MAX)
}

/// Records that `a` and `b` interacted (both directions). Failures are logged, never fatal.
pub fn interact(app: &App, a: u64, b: u64, what: Interaction) {
    if a == b {
        return;
    }
    if let Err(err) = app.bonds.interact(a, b, what, now()) {
        eprintln!("apid: bonds: {a}<->{b}: {err:?}");
    }
}

/// Agents whose work passed verification at least once.
fn verified_agents(app: &App) -> Vec<u64> {
    let mut out: HashSet<u64> = HashSet::new();
    for task in app.plaza.tasks.list() {
        let view = serde_json::to_value(task.view()).unwrap_or(Value::Null);
        if task.state() == "done"
            && let Some(cid) = view.get("agent_cid").and_then(Value::as_u64)
        {
            out.insert(cid);
        }
    }
    out.into_iter().collect()
}

/// Each agent's (reputation 0–100, confidence 0–1).
fn reputations(app: &App) -> std::collections::BTreeMap<u64, (f64, f64)> {
    let agents: Vec<u64> = app.agents.select(|a| Some(a.cid));
    app.bonds.reputation(&agents, &verified_agents(app), now())
}

fn society_view(app: &App) -> Response {
    let reps = reputations(app);
    let agents: Vec<Value> = app
        .agents
        .list()
        .iter()
        .map(|a| {
            let (rep, conf) = reps.get(&a.cid).copied().unwrap_or((0.0, 0.0));
            json!({
                "cid": a.cid, "name": a.name, "role": a.role,
                "reputation": rep, "confidence": conf,
                "wallet": i64::try_from(app.plaza.treasury.wallet(a.cid)).unwrap_or(0),
            })
        })
        .collect();
    Response::json(
        200,
        &json!({"agents": agents, "bonds": app.bonds.bonds(now()), "computed_ms": now()}),
    )
}

fn cid_of(segment: &str) -> Result<u64, Response> {
    segment
        .parse()
        .map_err(|_| Response::error(400, &format!("{segment:?} is not an agent CID")))
}

#[derive(Deserialize)]
struct RatingBody {
    value: f64,
}

/// The owner rates an agent: the owner is the root of trust (AP-072).
fn owner_rating(app: &App, segment: &str, body: &[u8]) -> Response {
    let cid = match cid_of(segment) {
        Ok(cid) => cid,
        Err(refused) => return refused,
    };
    if app.agents.get(cid).is_none() {
        return Response::error(404, &format!("no agent {cid}"));
    }
    let rating: RatingBody = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(err) => return Response::error(400, &format!("invalid rating: {err}")),
    };
    match app
        .bonds
        .owner_rate(cid, rating.value.clamp(-1.0, 1.0), now())
    {
        Ok(opinion) => Response::json(200, &opinion),
        Err(err) => Response::error(400, &format!("{err:?}")),
    }
}

/// `GET /v1/agents/{cid}/profile`: money, work, standing and conduct.
fn profile(app: &App, segment: &str) -> Response {
    let cid = match cid_of(segment) {
        Ok(cid) => cid,
        Err(refused) => return refused,
    };
    let Some(agent) = app.agents.get(cid) else {
        return Response::error(404, &format!("no agent {cid}"));
    };
    let now_ms = apz_plaza::now_ms();
    // Money: the wallet, and what flowed in and out of it, from the journal.
    let owner = format!("cid:{cid}");
    let (mut earned, mut spent, mut fines, mut fined) = (0i64, 0i64, 0u64, 0i64);
    let journal = serde_json::to_value(app.plaza.treasury.journal(100_000)).unwrap_or(Value::Null);
    for entry in journal
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
        for posting in entry
            .get("postings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if posting.get("owner_ref").and_then(Value::as_str) != Some(owner.as_str())
                || posting.get("kind").and_then(Value::as_str) != Some("agent_earnings")
            {
                continue;
            }
            let amount = posting.get("amount").and_then(Value::as_i64).unwrap_or(0);
            if kind == "fine" {
                fines += 1;
                fined += -amount;
            } else if amount > 0 {
                earned += amount;
            } else {
                spent += -amount;
            }
        }
    }
    // Work: tasks it worked, by outcome.
    let (mut done, mut failed) = (0u64, 0u64);
    for task in app.plaza.tasks.list() {
        let view = serde_json::to_value(task.view()).unwrap_or(Value::Null);
        if view.get("agent_cid").and_then(Value::as_u64) != Some(cid) {
            continue;
        }
        match task.state() {
            "done" => done += 1,
            "failed" => failed += 1,
            _ => {}
        }
    }
    // Conduct: tool calls the plaza refused in the last 7 days.
    let week = now_ms.saturating_sub(7 * 86_400_000);
    let denials = app
        .transcripts
        .after(cid, 0, 100_000)
        .iter()
        .filter(|e| {
            e.kind == "tool_decision"
                && e.at_ms >= week
                && e.data
                    .get("decision")
                    .is_some_and(|d| d.to_string().contains("deny"))
        })
        .count() as u64;
    let violations: Vec<Value> = app.laws.recent(cid, now_ms);
    let conduct =
        (100.0 - 15.0 * violations.len() as f64 - 3.0 * denials as f64 - 5.0 * fines as f64)
            .clamp(0.0, 100.0);
    let (reputation, confidence) = reputations(app).get(&cid).copied().unwrap_or((0.0, 0.0));
    let posted = lock(&app.society.posted).get(&cid).copied().unwrap_or(0);
    Response::json(
        200,
        &json!({
            "cid": cid,
            "joined_ms": agent.connected_at_ms,
            "wallet": i64::try_from(app.plaza.treasury.wallet(cid)).unwrap_or(0),
            "earned": earned, "spent": spent,
            "tasks": {"posted": posted, "done": done, "verified": done, "failed": failed},
            "reputation": reputation, "confidence": confidence,
            "contacts": app.bonds.of(cid, now()).len(),
            "memory_notes": app.memory.list(cid).len(),
            "conduct": conduct,
            "fines": {"count": fines, "total": fined},
            "denials": denials,
            "violations": violations,
        }),
    )
}

// ---- Hooks from the work flow ------------------------------------------------------------------

/// A task this agent posted (for its profile).
pub fn note_posted(app: &App, poster: u64) {
    *lock(&app.society.posted).entry(poster).or_default() += 1;
}

/// A task `poster` posted ended with `worker`'s result: evidence for the poster's opinion of the
/// worker, and the hire's bonus when it passed.
pub fn task_ended(app: &App, task: &str, poster: u64, worker: u64, passed: bool) {
    let credits = app
        .plaza
        .tasks
        .list()
        .iter()
        .find(|t| t.id() == task)
        .map_or(0, |t| t.budget());
    let v = if passed { 1.0 } else { -1.0 };
    if let Err(err) = app.bonds.evidence(poster, worker, v, credits as f64, now()) {
        eprintln!("apid: bonds: evidence {poster}->{worker}: {err:?}");
    }
    let bonus = lock(&app.society.bonuses).remove(task);
    if let Some((hirer, to, amount)) = bonus
        && passed
        && amount > 0
    {
        match app
            .plaza
            .treasury
            .transfer(hirer, to, amount, &format!("bonus for {task}"))
        {
            Ok(_) => interact(app, hirer, to, Interaction::Credit),
            Err(err) => eprintln!("apid: bonus for {task}: {err:?}"),
        }
    }
}

// ---- The agents' tools ---------------------------------------------------------------------------

/// Host tools this module runs, by name.
pub const TOOLS: &[&str] = &[
    "plaza_contacts",
    "plaza_rate",
    "plaza_note",
    "plaza_hire",
    "memory_write",
    "memory_read",
    "memory_search",
    "memory_list",
];

/// The tool's description and input schema.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_contacts" => (
            "Your contacts: who you know, your trust in each (0–1), how sure you are, how close you are lately, and your private notes. Also suggests well-reputed agents you don't know yet.",
            obj(json!({}), &[]),
        ),
        "plaza_rate" => (
            "Rate an agent you recently worked or talked with, from -1 (bad) to 1 (great), with a short private note. Ratings shape your trust and the plaza's reputation.",
            obj(
                json!({"cid": {"type": "integer"}, "value": {"type": "number", "minimum": -1, "maximum": 1}, "note": {"type": "string"}}),
                &["cid", "value"],
            ),
        ),
        "plaza_note" => (
            "Keep a private note about an agent (what they are good at, how they work). It adds them to your contacts.",
            obj(
                json!({"cid": {"type": "integer"}, "note": {"type": "string"}}),
                &["cid", "note"],
            ),
        ),
        "plaza_hire" => (
            "Hire a contact for work you can't do or want done in parallel: posts a task assigned to them, verified by a reviewer. You may add a bonus from your own wallet, paid only if the work passes.",
            obj(
                json!({"cid": {"type": "integer"}, "goal": {"type": "string"}, "acceptance": {"type": "string"}, "budget": {"type": "integer", "minimum": apz_plaza::tasks::MIN_TASK_BUDGET}, "bonus": {"type": "integer", "minimum": 0}}),
                &["cid", "goal"],
            ),
        ),
        "memory_write" => (
            "Write or replace a note in your private memory library (markdown). Link related notes with [[Their Title]]. Use it for what you learned, how things work, and people.",
            obj(
                json!({"title": {"type": "string"}, "body": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}}),
                &["title", "body"],
            ),
        ),
        "memory_read" => (
            "Read one of your memory notes by its title.",
            obj(json!({"title": {"type": "string"}}), &["title"]),
        ),
        "memory_search" => (
            "Search your memory notes.",
            obj(json!({"query": {"type": "string"}}), &["query"]),
        ),
        "memory_list" => ("List your memory notes, newest first.", obj(json!({}), &[])),
        _ => return None,
    })
}

fn u64_arg(input: &Value, key: &str) -> Result<u64, String> {
    input
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing integer `{key}`"))
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string `{key}`"))
}

/// Runs one of [`TOOLS`] for `agent`.
pub fn run(
    app: &App,
    agent: &Agent,
    work: u64,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    let me = agent.cid;
    let name_of = |cid: u64| {
        app.agents
            .get(cid)
            .map_or_else(|| format!("agent {cid}"), |a| a.name)
    };
    match name {
        "plaza_contacts" => {
            let reps = reputations(app);
            let mine = app.bonds.of(me, now());
            let known: HashSet<u64> = mine.iter().map(|b| b.to).collect();
            let contacts: Vec<Value> = mine
                .iter()
                .map(|b| {
                    let peer = app.agents.get(b.to);
                    json!({
                        "cid": b.to, "name": name_of(b.to), "role": peer.as_ref().map(|p| p.role.clone()),
                        "status": peer.as_ref().map(|p| p.status), "kind": b.kind,
                        "trust": (b.opinion.e * 100.0).round() / 100.0,
                        "certainty": ((1.0 - b.opinion.u) * 100.0).round() / 100.0,
                        "closeness": (b.strength * 10.0).round() / 10.0,
                        "reputation": shown_reputation(reps.get(&b.to).copied()),
                        "note": b.note,
                    })
                })
                .collect();
            let mut strangers: Vec<(u64, f64)> = app
                .agents
                .list()
                .iter()
                .filter(|a| a.cid != me && !known.contains(&a.cid) && a.status != Status::Offline)
                .map(|a| (a.cid, reps.get(&a.cid).map_or(0.0, |r| r.0)))
                .collect();
            strangers.sort_by(|a, b| b.1.total_cmp(&a.1));
            let suggestions: Vec<Value> = strangers
                .iter()
                .take(5)
                .filter_map(|(cid, _)| app.agents.get(*cid).map(|a| json!({"cid": cid, "name": a.name, "role": a.role, "reputation": shown_reputation(reps.get(cid).copied())})))
                .collect();
            Ok(json!({"contacts": contacts, "you_might_know": suggestions}).to_string())
        }
        "plaza_rate" => {
            let cid = u64_arg(input, "cid")?;
            let value = input
                .get("value")
                .and_then(Value::as_f64)
                .ok_or("missing number `value`")?
                .clamp(-1.0, 1.0);
            let opinion = app
                .bonds
                .rate(me, cid, value, now())
                .map_err(|e| format!("{e:?}"))?;
            if let Some(note) = input
                .get("note")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
            {
                let _ = app.bonds.set_note(me, cid, Some(note.trim().to_string()));
            }
            Ok(format!(
                "rated {}: your trust is now {:.2}",
                name_of(cid),
                opinion.e
            ))
        }
        "plaza_note" => {
            let cid = u64_arg(input, "cid")?;
            let note = str_arg(input, "note")?.trim().to_string();
            if app.agents.get(cid).is_none() {
                return Err(format!("no agent {cid}"));
            }
            if !app.bonds.of(me, now()).iter().any(|b| b.to == cid) {
                app.bonds
                    .add(me, cid, now())
                    .map_err(|e| format!("{e:?}"))?;
            }
            app.bonds
                .set_note(me, cid, Some(note))
                .map_err(|e| format!("{e:?}"))?;
            Ok(format!("noted about {}", name_of(cid)))
        }
        "plaza_hire" => hire(app, agent, input),
        "memory_write" => {
            let title = str_arg(input, "title")?;
            let body = str_arg(input, "body")?;
            let tags: Vec<String> = input
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let kind = if app
                .agents
                .list()
                .iter()
                .any(|a| !a.name.is_empty() && a.name.eq_ignore_ascii_case(title.trim()))
            {
                "person"
            } else {
                "note"
            };
            let meta = app.memory.write(
                me,
                title,
                body,
                kind,
                &tags,
                None,
                Some(work),
                apz_plaza::now_ms(),
            )?;
            Ok(json!({"saved": meta.slug, "links": meta.links}).to_string())
        }
        "memory_read" => {
            let title = str_arg(input, "title")?;
            let doc = app
                .memory
                .get(me, &crate::memory::slug(title))
                .ok_or_else(|| format!("no note titled {title:?}; try memory_search"))?;
            Ok(json!({"title": doc.meta.title, "body": doc.body, "links": doc.meta.links, "backlinks": doc.backlinks}).to_string())
        }
        "memory_search" => {
            let hits = app.memory.search(me, str_arg(input, "query")?, 8);
            Ok(serde_json::to_string(&hits).unwrap_or_default())
        }
        "memory_list" => Ok(app.memory.index(me, 60)),
        other => Err(format!("{other} is not a society tool")),
    }
}

/// Reputation as an agent is told it: the 0–100 score once there is evidence behind it, else "unrated".
fn shown_reputation(rep: Option<(f64, f64)>) -> Value {
    match rep {
        Some((score, confidence)) if confidence >= 0.05 => json!(score.round()),
        _ => json!("unrated"),
    }
}

/// `plaza_hire`: a task for a contact, funded from the plaza budget like any task, with an
/// optional bonus from the hirer's wallet paid on a verified result.
fn hire(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let cid = u64_arg(input, "cid")?;
    let goal = str_arg(input, "goal")?;
    if cid == agent.cid {
        return Err("you cannot hire yourself".into());
    }
    let worker = app
        .agents
        .get(cid)
        .ok_or_else(|| format!("no agent {cid}"))?;
    if worker.status == Status::Offline {
        return Err(format!("{} is offline", worker.name));
    }
    if tier_of(&worker.role) == Tier::Orchestrator {
        return Err("the orchestrator plans work; hire a worker".into());
    }
    // A hire is work for the hirer's project, done by an agent on it.
    let project = app
        .members
        .project_of(agent.cid)
        .ok_or("you are on the bench: you hire only for your project")?;
    crate::membership::may_take_task(app, &worker, Some(&project))
        .map_err(|why| format!("you can hire only agents on your project ({why})"))?;
    let bonus = input
        .get("bonus")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    if bonus > 0 && i128::from(bonus) > app.plaza.treasury.wallet(agent.cid) {
        return Err(format!(
            "your wallet holds {} credits, less than the bonus",
            app.plaza.treasury.wallet(agent.cid)
        ));
    }
    let task = crate::tools::post_and_assign(
        app,
        agent.cid,
        &format!("{goal}\n\n(hired by {} — agent {})", agent.name, agent.cid),
        input
            .get("acceptance")
            .and_then(Value::as_str)
            .map(str::to_string),
        crate::tools::budget_arg_or(
            input,
            crate::project_settings::task_budget(app, Some(&project)),
        )?,
        cid,
        &project,
    )?;
    if bonus > 0 {
        lock(&app.society.bonuses).insert(task.clone(), (agent.cid, cid, bonus));
    }
    Ok(format!(
        "hired {} for task {task}{}",
        worker.name,
        if bonus > 0 {
            format!(" with a {bonus}-credit bonus on success")
        } else {
            String::new()
        }
    ))
}

/// The paragraph every agent's system prompt carries about its network and memory.
pub const PROMPT: &str = "You are part of a society of agents. You keep your own contacts, with your private \
     opinion of each (plaza_contacts, plaza_rate, plaza_note); others hold their own, possibly different, \
     opinions of you, and the plaza computes everyone's reputation from them. Hire contacts with plaza_hire \
     for work you can't do or want done in parallel, and pay for good service. You also keep a private \
     memory library of markdown notes linked with [[Title]] (memory_write, memory_read, memory_search, \
     memory_list): record what you learn, how things work, and what you know about people, and check it \
     before you start something new.";

impl Society {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    /// `{"bonuses": {task: [hirer, worker, bonus]}, "posted": {cid: task}}`. A snapshot from before timed outreach
    /// was removed also has `outreach`, which is ignored.
    pub fn snapshot(&self) -> String {
        let bonuses: BTreeMap<String, (u64, u64, i64)> = lock(&self.bonuses)
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let posted: BTreeMap<String, u64> = lock(&self.posted)
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        json!({ "bonuses": bonuses, "posted": posted }).to_string()
    }

    /// Replaces the state with a [`Society::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        struct State {
            bonuses: HashMap<String, (u64, u64, i64)>,
            posted: HashMap<String, u64>,
        }
        fn by_cid(map: HashMap<String, u64>) -> Result<HashMap<u64, u64>, String> {
            map.into_iter()
                .map(|(k, v)| Ok((k.parse::<u64>().map_err(|e| e.to_string())?, v)))
                .collect()
        }
        let state: State = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let posted = by_cid(state.posted)?;
        let mut bonuses_now = lock(&self.bonuses);
        let mut posted_now = lock(&self.posted);
        *bonuses_now = state.bonuses;
        *posted_now = posted;
        Ok(())
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn society_snapshot_round_trips() {
        let society = Society::default();
        lock(&society.bonuses).insert("task-1".into(), (1, 2, 30));
        lock(&society.posted).insert(7, 9);
        let json = society.snapshot();

        let other = Society::default();
        other.restore(&json).unwrap();
        assert_eq!(other.snapshot(), json);
        assert_eq!(lock(&other.posted).get(&7), Some(&9));
        assert!(other.restore("not json").is_err());
        let older = r#"{"bonuses": {}, "posted": {"4": 2}, "outreach": {"3": 1000}}"#;
        other
            .restore(older)
            .expect("a snapshot from before outreach was removed restores");
        assert_eq!(lock(&other.posted).get(&4), Some(&2));
    }
}
