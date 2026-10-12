//! The plaza's toolbox: what an agent can reach right now, and what agents found for themselves.
//!
//! Nothing here names an outside tool. `plaza_toolbox` lists, from live state, the plaza's own
//! tools, the connectors the owner enabled, the tools agents built (the library), and the
//! discoveries agents recorded — software, services and techniques they found on the open
//! internet, installed in their own VMs and judged. `plaza_toolbox_share` records one, so what
//! one agent learns reaches every other. The owner sees the discoveries at `GET /v1/toolbox`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::Agent;
use crate::state::App;

/// The host tools this module serves.
pub const TOOLS: &[&str] = &["plaza_toolbox", "plaza_toolbox_share"];

/// The most discoveries kept; the oldest unendorsed one goes past it.
pub const MAX_DISCOVERIES: usize = 500;

/// One thing an agent found and judged: a CLI, a library, a service, an API or a technique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// `d<seq>`.
    pub id: String,
    /// What it is called, as its makers call it.
    pub name: String,
    /// "cli" | "library" | "service" | "api" | "technique".
    pub kind: String,
    /// What it is good for, in one or two sentences.
    pub what_for: String,
    /// How to get and use it from an agent's VM: install line, entry command, gotchas.
    pub how: String,
    /// The finder's verdict after trying it: what worked, what did not.
    pub verdict: String,
    /// Where it lives (homepage or repository), when known.
    #[serde(default)]
    pub url: Option<String>,
    /// Free-form tags for search ("video", "audio", "pdf", ...), chosen by agents.
    #[serde(default)]
    pub tags: Vec<String>,
    pub by_cid: u64,
    pub by: String,
    pub at_ms: u64,
    /// Agents that tried it too and vouch for it (cids), the finder excluded.
    #[serde(default)]
    pub endorsed_by: Vec<u64>,
}

/// The discoveries, kept in `<state-dir>/toolbox.json`.
pub struct Store {
    path: PathBuf,
    items: Mutex<Vec<Discovery>>,
}

impl Store {
    pub fn load(state_dir: &Path) -> Self {
        // contract: F-T1 reads the file (missing or bad → empty).
        Self { path: state_dir.join("toolbox.json"), items: Mutex::new(Vec::new()) }
    }

    /// All discoveries, newest first.
    pub fn list(&self) -> Vec<Discovery> {
        // contract: F-T1
        let _ = &self.path;
        self.items.lock().map(|items| items.clone()).unwrap_or_default()
    }

    /// Records a discovery, or — when the same agent already shared one with the same name
    /// (case-insensitive) — updates it. When another agent shared that name, `by_cid` is added
    /// to its `endorsed_by` and the newer verdict is appended to its verdict instead.
    pub fn share(&self, _by_cid: u64, _by: &str, _input: &Value) -> Result<Discovery, String> {
        // contract: F-T1
        Err("the toolbox is not available yet".to_string())
    }

    /// Discoveries matching every word of `query` (case-insensitive, over name, kind, what_for,
    /// how, verdict and tags), best first: more endorsements, then newer.
    pub fn search(&self, _query: &str) -> Vec<Discovery> {
        // contract: F-T1
        Vec::new()
    }
}

/// The tools' specs, in the same shape as `society::spec`.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_toolbox" => (
            "Survey what you can use before you build anything: the plaza's tools, the connectors the owner enabled (external services, keys held by the plaza), tools other agents built, and software, services and techniques other agents found on the internet, tried in their VMs and judged. Give `query` (words, e.g. what the job needs) to search; omit it for the whole toolbox.",
            obj(json!({"query": {"type": "string"}}), &[]),
        ),
        "plaza_toolbox_share" => (
            "Record something you found and tried (a CLI, a library, a service, an API or a technique) so every agent can use it: what it is for, how to get and run it from a VM, and your honest verdict. Sharing the same name again updates yours; sharing one another agent found endorses it.",
            obj(
                json!({
                    "name": {"type": "string"},
                    "kind": {"type": "string", "enum": ["cli", "library", "service", "api", "technique"]},
                    "what_for": {"type": "string"},
                    "how": {"type": "string"},
                    "verdict": {"type": "string"},
                    "url": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 8}
                }),
                &["name", "kind", "what_for", "how", "verdict"],
            ),
        ),
        _ => return None,
    })
}

/// Runs `plaza_toolbox` or `plaza_toolbox_share` for `agent`.
pub fn run(_app: &App, _agent: &Agent, _name: &str, _input: &Value) -> Result<String, String> {
    // contract: F-T2 (the live survey) and F-T1 (share)
    Err("the toolbox is not available yet".to_string())
}

pub fn routes(router: Router<App>) -> Router<App> {
    router.get("/v1/toolbox", list_route)
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &app.toolbox.list())
}
