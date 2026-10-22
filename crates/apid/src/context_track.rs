//! What each chat holds: per agent and work item, the latest model call's context (used against the model's
//! window, split into the six context parts) and the tool calls of that work (calls, errors, result sizes), for
//! the consoles' context bar and its breakdown (`GET /v1/agents/{cid}/context`,
//! `GET /v1/chats/{id}/context`). Kept in memory: a restart starts the bars at the next call.
//!
//! It also holds a chat's model choice for one message (`POST /v1/agents/{cid}/chat {model}`), which
//! [`chat_model`] gives the work item that message queued; a chat's own `model` applies to every work item it queued.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Serialize;
use serde_json::{Value, json};

use crate::state::App;

/// Bytes per token, the same estimate the context split and the compactions use.
const BYTES_PER_TOKEN: u64 = 4;
/// The least time between two `context` events of one agent.
const EVENT_EVERY: Duration = Duration::from_secs(1);
/// The most work items whose snapshots are kept per agent (the oldest go first).
const KEEP_PER_AGENT: usize = 16;

/// One tool's tally in a work item.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ToolTally {
    pub calls: u64,
    pub errors: u64,
    /// The tokens its results added to the context: their bytes over [`BYTES_PER_TOKEN`].
    pub result_tokens: u64,
}

/// A chat's context: the latest call's, and the work item's tool calls so far.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub cid: u64,
    pub work: u64,
    pub model: String,
    /// The model's context window in tokens, when known.
    pub window: Option<u64>,
    /// The latest call's context: its input, cache reads and cache writes.
    pub used: u64,
    /// The latest call's answer.
    pub output: u64,
    /// `used` by context part: system, tools, history, tool_results, attachments, summary.
    pub parts: BTreeMap<String, u64>,
    /// Model calls in this work item.
    pub calls: u64,
    /// Compactions of the agent's conversation during this work item.
    pub compactions: u64,
    /// The work item's tool calls, by tool name.
    pub tools: BTreeMap<String, ToolTally>,
    pub updated_ms: u64,
}

#[derive(Default)]
pub struct Tracker {
    snapshots: Mutex<HashMap<(u64, u64), Snapshot>>,
    /// Each agent's work items with a snapshot, oldest first.
    order: Mutex<HashMap<u64, Vec<u64>>>,
    published: Mutex<HashMap<u64, Instant>>,
    /// A one-message model choice, by work item: `<provider>/<model>[@effort]`.
    overrides: Mutex<HashMap<u64, String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Tracker {
    fn with(&self, cid: u64, work: u64, change: impl FnOnce(&mut Snapshot)) -> Snapshot {
        let mut snapshots = lock(&self.snapshots);
        let snap = snapshots.entry((cid, work)).or_insert_with(|| Snapshot {
            cid,
            work,
            ..Default::default()
        });
        change(snap);
        snap.updated_ms = apz_plaza::now_ms();
        let out = snap.clone();
        drop(snapshots);
        let mut order = lock(&self.order);
        let works = order.entry(cid).or_default();
        if !works.contains(&work) {
            works.push(work);
            if works.len() > KEEP_PER_AGENT {
                let old = works.remove(0);
                lock(&self.snapshots).remove(&(cid, old));
                lock(&self.overrides).remove(&old);
            }
        }
        out
    }

    /// A model call finished: its model, window, context (`used`, split into `parts`) and answer.
    pub fn on_model(
        &self,
        cid: u64,
        work: u64,
        model: &str,
        window: Option<u64>,
        used: u64,
        output: u64,
        parts: &[(String, u64)],
    ) -> Snapshot {
        self.with(cid, work, |s| {
            s.model = model.to_string();
            s.window = window;
            s.used = used;
            s.output = output;
            s.parts = parts.iter().cloned().collect();
            s.calls += 1;
        })
    }

    /// A tool reported: its name, whether it succeeded, and the size of its result in bytes.
    pub fn on_tool(
        &self,
        cid: u64,
        work: u64,
        name: &str,
        ok: bool,
        result_bytes: usize,
    ) -> Snapshot {
        self.with(cid, work, |s| {
            let t = s.tools.entry(name.to_string()).or_default();
            t.calls += 1;
            if !ok {
                t.errors += 1;
            }
            t.result_tokens += result_bytes as u64 / BYTES_PER_TOKEN;
        })
    }

    /// The agent's conversation was compacted during `work`.
    pub fn on_compaction(&self, cid: u64, work: u64) {
        self.with(cid, work, |s| s.compactions += 1);
    }

    /// The snapshot of `cid`'s `work`, or of its latest work item.
    pub fn get(&self, cid: u64, work: Option<u64>) -> Option<Snapshot> {
        let work = match work {
            Some(w) => w,
            None => *lock(&self.order).get(&cid)?.last()?,
        };
        lock(&self.snapshots).get(&(cid, work)).cloned()
    }

    /// The latest snapshot among `works` (a chat's work items, oldest first).
    pub fn latest_of(&self, cid: u64, works: &[u64]) -> Option<Snapshot> {
        let snapshots = lock(&self.snapshots);
        works
            .iter()
            .rev()
            .find_map(|w| snapshots.get(&(cid, *w)).cloned())
    }

    /// Whether a `context` event for `cid` may go out now (at most one per [`EVENT_EVERY`]).
    fn may_publish(&self, cid: u64) -> bool {
        let mut published = lock(&self.published);
        let now = Instant::now();
        match published.get(&cid) {
            Some(at) if now.duration_since(*at) < EVENT_EVERY => false,
            _ => {
                published.insert(cid, now);
                true
            }
        }
    }

    /// A one-message model choice for `work`.
    pub fn set_override(&self, work: u64, model: String) {
        lock(&self.overrides).insert(work, model);
    }

    fn override_of(&self, work: u64) -> Option<String> {
        lock(&self.overrides).get(&work).cloned()
    }
}

/// Publishes `snap` as a `context` event, at most once a second per agent.
pub fn publish(app: &App, snap: &Snapshot) {
    if app.context.may_publish(snap.cid) {
        app.bus.publish(
            "context",
            &json!({ "cid": snap.cid, "work": snap.work, "used": snap.used, "window": snap.window }),
        );
    }
}

/// The model a chat chose for `work`: a one-message choice, else the model of the chat `work` answers. None: the
/// agent's model applies.
pub fn chat_model(app: &App, cid: u64, work: u64) -> Option<String> {
    if let Some(m) = app.context.override_of(work) {
        return Some(m);
    }
    let item = app.work.current(cid).filter(|item| item.id == work)?;
    let chat = item.thread.strip_prefix("chat:")?;
    app.chats.get(chat)?.model
}

/// A snapshot as the consoles read it: the parts in their fixed order, and the share of the window used.
fn view(snap: &Snapshot) -> Value {
    let parts: Vec<Value> = crate::internal::CONTEXT_PARTS
        .iter()
        .map(|name| json!({ "part": name, "tokens": snap.parts.get(*name).copied().unwrap_or(0) }))
        .collect();
    let mut tools: Vec<Value> = snap
        .tools
        .iter()
        .map(|(name, t)| json!({ "tool": name, "calls": t.calls, "errors": t.errors, "result_tokens": t.result_tokens }))
        .collect();
    tools.sort_by_key(|t| std::cmp::Reverse(t["result_tokens"].as_u64().unwrap_or(0)));
    let fill = snap
        .window
        .filter(|w| *w > 0)
        .map(|w| (snap.used as f64 / w as f64).min(1.0));
    json!({
        "cid": snap.cid, "work": snap.work, "model": snap.model, "window": snap.window, "used": snap.used,
        "output": snap.output, "fill": fill, "parts": parts, "tools": tools, "calls": snap.calls,
        "compactions": snap.compactions, "updated_ms": snap.updated_ms,
    })
}

/// The empty snapshot of an agent with no call yet: its model and window, nothing used.
fn empty(app: &App, cid: u64) -> Snapshot {
    let model = app
        .agents
        .get(cid)
        .map(|a| crate::agents::resolved_model(app, &a).0)
        .unwrap_or_default();
    Snapshot {
        cid,
        window: crate::providers::context_window(app, &model),
        model,
        ..Default::default()
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agents/{cid}/context", agent_route)
        .doc(RouteDoc::new("An agent's chat context: used against the window, by part, and its tool calls (?work=)").errors(&[400, 404]))
        .get("/v1/chats/{id}/context", chat_route)
        .doc(RouteDoc::new("A chat's context: the orchestrator's (or its planner's) turns on it").errors(&[404]))
}

/// The context of VM `cid` (its last call's, or of the work item `work`), as the consoles read it.
pub fn of_cid(app: &App, cid: u64, work: Option<u64>) -> Value {
    view(
        &app.context
            .get(cid, work)
            .unwrap_or_else(|| empty(app, cid)),
    )
}

fn agent_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Ok(cid) = p.get("cid").parse::<u64>() else {
        return Response::error(400, "cid must be a number");
    };
    if app.agents.get(cid).is_none() {
        return Response::error(404, "unknown agent");
    }
    let work = match req.query_param("work").map(str::parse::<u64>) {
        None => None,
        Some(Ok(w)) => Some(w),
        Some(Err(_)) => return Response::error(400, "work must be a number"),
    };
    let snap = app
        .context
        .get(cid, work)
        .unwrap_or_else(|| empty(app, cid));
    Response::json(200, &view(&snap))
}

fn chat_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let Some(chat) = app.chats.get(p.get("id")) else {
        return Response::error(404, "no such chat");
    };
    let cid = crate::work_loop::chat_cid(app, &chat).unwrap_or(0);
    let mut snap = app
        .context
        .latest_of(cid, &chat.works)
        .unwrap_or_else(|| empty(app, cid));
    if let Some(model) = &chat.model
        && snap.calls == 0
    {
        snap.window = crate::providers::context_window(app, model);
        snap.model = model.clone();
    }
    Response::json(200, &view(&snap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_item_tallies_its_calls_and_tools() {
        let t = Tracker::default();
        let parts = vec![("system".to_string(), 100), ("history".to_string(), 900)];
        t.on_model(
            7,
            1,
            "anthropic/claude-sonnet-5-5",
            Some(200_000),
            1000,
            50,
            &parts,
        );
        t.on_tool(7, 1, "read", true, 4000);
        t.on_tool(7, 1, "read", false, 40);
        t.on_compaction(7, 1);
        let s = t.on_model(
            7,
            1,
            "anthropic/claude-sonnet-5-5",
            Some(200_000),
            2000,
            60,
            &parts,
        );
        assert_eq!((s.used, s.calls, s.compactions), (2000, 2, 1));
        assert_eq!(
            s.tools["read"],
            ToolTally {
                calls: 2,
                errors: 1,
                result_tokens: 1010
            }
        );
        assert_eq!(t.get(7, None).unwrap().work, 1);
        let v = view(&s);
        assert_eq!(v["fill"], 0.01);
        assert_eq!(v["parts"][0]["part"], "system");
    }

    #[test]
    fn old_work_items_are_dropped_and_events_are_throttled() {
        let t = Tracker::default();
        for w in 0..(KEEP_PER_AGENT as u64 + 3) {
            t.on_tool(1, w, "x", true, 0);
        }
        assert!(t.get(1, Some(0)).is_none());
        assert!(t.get(1, Some(KEEP_PER_AGENT as u64 + 2)).is_some());
        assert_eq!(t.latest_of(1, &[3, 5]).unwrap().work, 5);
        assert!(t.may_publish(1));
        assert!(!t.may_publish(1));
        assert!(t.may_publish(2));
    }
}
