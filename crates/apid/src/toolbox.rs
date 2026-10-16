//! The plaza's toolbox: what an agent can reach right now, and what agents found for themselves.
//!
//! Nothing here names an outside tool. `plaza_toolbox` lists, from live state, the plaza's own
//! tools, the connectors the owner enabled, the tools agents built (the library), and the
//! discoveries agents recorded — software, services and techniques they found on the open
//! internet, installed in their own VMs and judged. `plaza_toolbox_share` records one, so what
//! one agent learns reaches every other. The owner sees the discoveries at `GET /v1/toolbox`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::state::App;

/// The host tools this module serves.
pub const TOOLS: &[&str] = &["plaza_toolbox", "plaza_toolbox_share"];

/// The most discoveries kept; the oldest unendorsed one goes past it.
pub const MAX_DISCOVERIES: usize = 500;

const NAME_MAX: usize = 80;
const WHAT_FOR_MAX: usize = 400;
const HOW_MAX: usize = 2000;
const VERDICT_MAX: usize = 1000;
/// A verdict grows by one line per endorsing agent; past this, its oldest text goes.
const VERDICT_KEEP: usize = 4000;
const TAGS_MAX: usize = 8;
/// The most characters of the owner's reason for flagging a discovery.
pub const REASON_MAX: usize = 500;
const KINDS: &[&str] = &["cli", "library", "service", "api", "technique"];

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
    /// The owner's flag: why this should not be used (shown to agents beside it), or None.
    #[serde(default)]
    pub flagged: Option<String>,
}

/// The discoveries, kept in `<state-dir>/toolbox.json`.
pub struct Store {
    path: PathBuf,
    items: Mutex<Vec<Discovery>>,
}

impl Store {
    /// Reads `<state_dir>/toolbox.json`; missing or unparsable means empty.
    pub fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("toolbox.json");
        let items = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<Discovery>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    /// All discoveries, newest first.
    pub fn list(&self) -> Vec<Discovery> {
        let mut items = self
            .items
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        items.sort_by_key(|a| std::cmp::Reverse(a.at_ms));
        items
    }

    /// Writes the whole list to a temp file next to `path`, then renames it over `path`.
    /// Errors are ignored: the in-memory list stays the truth for this process.
    fn save(&self, items: &[Discovery]) {
        let Ok(json) = serde_json::to_string_pretty(items) else {
            return;
        };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// Records a discovery, or — when the same agent already shared one with the same name
    /// (case-insensitive) — updates it. When another agent shared that name, `by_cid` is added
    /// to its `endorsed_by` and the newer verdict is appended to its verdict instead.
    pub fn share(&self, by_cid: u64, by: &str, input: &Value) -> Result<Discovery, String> {
        let text = |key: &str, max: usize| -> Result<String, String> {
            let s = input.get(key).and_then(Value::as_str).unwrap_or("").trim();
            if s.is_empty() {
                return Err(format!("{key} is required"));
            }
            if s.chars().count() > max {
                return Err(format!("{key} is over {max} characters"));
            }
            Ok(s.to_string())
        };
        let name = text("name", NAME_MAX)?;
        let kind = input
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if !KINDS.contains(&kind.as_str()) {
            return Err(format!("kind must be one of {}", KINDS.join(", ")));
        }
        let what_for = text("what_for", WHAT_FOR_MAX)?;
        let how = text("how", HOW_MAX)?;
        let verdict = text("verdict", VERDICT_MAX)?;
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        let mut tags: Vec<String> = Vec::new();
        if let Some(raw) = input.get("tags").and_then(Value::as_array) {
            if raw.len() > TAGS_MAX {
                return Err(format!("tags takes at most {TAGS_MAX} entries"));
            }
            for tag in raw
                .iter()
                .filter_map(Value::as_str)
                .map(|t| t.trim().to_lowercase())
            {
                if !tag.is_empty() && !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
        }

        let key = name.to_lowercase();
        let now = now_ms();
        let mut items = self
            .items
            .lock()
            .map_err(|_| "the toolbox is locked".to_string())?;
        let result = if let Some(i) = items
            .iter()
            .position(|d| d.by_cid == by_cid && d.name.to_lowercase() == key)
        {
            let d = &mut items[i];
            d.name = name;
            d.kind = kind;
            d.what_for = what_for;
            d.how = how;
            d.verdict = verdict;
            if url.is_some() {
                d.url = url;
            }
            if !tags.is_empty() {
                d.tags = tags;
            }
            d.by = by.to_string();
            d.at_ms = now;
            d.clone()
        } else if let Some(i) = items.iter().position(|d| d.name.to_lowercase() == key) {
            let d = &mut items[i];
            if !d.endorsed_by.contains(&by_cid) {
                d.endorsed_by.push(by_cid);
            }
            d.verdict.push_str(&format!("\n— {by}: {verdict}"));
            let excess = d.verdict.chars().count().saturating_sub(VERDICT_KEEP);
            if excess > 0 {
                d.verdict = d.verdict.chars().skip(excess).collect();
            }
            for tag in tags {
                if !d.tags.contains(&tag) {
                    d.tags.push(tag);
                }
            }
            d.at_ms = now;
            d.clone()
        } else {
            let largest = items
                .iter()
                .filter_map(|d| d.id.strip_prefix('d')?.parse::<u64>().ok())
                .max()
                .unwrap_or(0);
            if items.len() >= MAX_DISCOVERIES {
                // Drop the oldest with no endorsements; if every one is endorsed, the oldest overall.
                let unendorsed = items
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| d.endorsed_by.is_empty())
                    .min_by_key(|(_, d)| d.at_ms)
                    .map(|(i, _)| i);
                let oldest = unendorsed.or_else(|| {
                    items
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, d)| d.at_ms)
                        .map(|(i, _)| i)
                });
                if let Some(i) = oldest {
                    items.remove(i);
                }
            }
            let d = Discovery {
                id: format!("d{}", largest + 1),
                name,
                kind,
                what_for,
                how,
                verdict,
                url,
                tags,
                by_cid,
                by: by.to_string(),
                at_ms: now,
                endorsed_by: Vec::new(),
                flagged: None,
            };
            items.push(d.clone());
            d
        };
        self.save(&items);
        Ok(result)
    }

    /// Discoveries matching every word of `query` (case-insensitive, over name, kind, what_for,
    /// how, verdict and tags), best first: more endorsements, then newer.
    pub fn search(&self, query: &str) -> Vec<Discovery> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let items = self
            .items
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        let mut hits: Vec<Discovery> = items
            .into_iter()
            .filter(|d| {
                let mut fields: Vec<String> = [&d.name, &d.kind, &d.what_for, &d.how, &d.verdict]
                    .iter()
                    .map(|f| f.to_lowercase())
                    .collect();
                fields.extend(d.tags.iter().map(|t| t.to_lowercase()));
                words
                    .iter()
                    .all(|w| fields.iter().any(|f| f.contains(w.as_str())))
            })
            .collect();
        hits.sort_by(|a, b| {
            b.endorsed_by
                .len()
                .cmp(&a.endorsed_by.len())
                .then(b.at_ms.cmp(&a.at_ms))
        });
        hits
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
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
pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "plaza_toolbox_share" => {
            let who = if agent.name.is_empty() {
                agent.role.clone()
            } else {
                agent.name.clone()
            };
            let item = app.toolbox.share(agent.cid, &who, input)?;
            serde_json::to_string(&item).map_err(|e| e.to_string())
        }
        _ => {
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            Ok(crate::toolbox_survey::survey(app, agent, query))
        }
    }
}

impl Store {
    /// Removes a discovery (the owner). Err when there is none.
    pub fn remove(&self, id: &str) -> Result<Discovery, String> {
        let mut items = self
            .items
            .lock()
            .map_err(|_| "the toolbox is locked".to_string())?;
        let i = items
            .iter()
            .position(|d| d.id == id)
            .ok_or_else(|| format!("no discovery {id}"))?;
        let removed = items.remove(i);
        self.save(&items);
        Ok(removed)
    }

    /// Flags a discovery with the owner's reason (trimmed), or clears the flag with None or a
    /// blank reason. Err when there is no such discovery or the reason is over `REASON_MAX`.
    pub fn flag(&self, id: &str, reason: Option<String>) -> Result<Discovery, String> {
        let reason = reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty());
        if reason
            .as_deref()
            .is_some_and(|r| r.chars().count() > REASON_MAX)
        {
            return Err(format!("reason is over {REASON_MAX} characters"));
        }
        let mut items = self
            .items
            .lock()
            .map_err(|_| "the toolbox is locked".to_string())?;
        let d = items
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("no discovery {id}"))?;
        d.flagged = reason;
        let flagged = d.clone();
        self.save(&items);
        Ok(flagged)
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/toolbox", list_route)
        .delete("/v1/toolbox/{id}", remove_route)
        .post("/v1/toolbox/{id}/flag", flag_route)
}

/// The owner's body for a flag: `{reason: string | null}`; null or blank clears the flag.
#[derive(Deserialize)]
struct FlagBody {
    reason: Option<String>,
}

fn remove_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.toolbox.remove(p.get("id")) {
        Ok(item) => {
            app.bus.publish("toolbox", &item);
            Response::json(200, &item)
        }
        Err(e) => Response::error(404, &e),
    }
}

fn flag_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let body: FlagBody = match req.json::<FlagBody>() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    if !app.toolbox.list().iter().any(|d| d.id == id) {
        return Response::error(404, &format!("no discovery {id}"));
    }
    match app.toolbox.flag(id, body.reason) {
        Ok(item) => {
            app.bus.publish("toolbox", &item);
            Response::json(200, &item)
        }
        Err(e) => Response::error(400, &e),
    }
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &app.toolbox.list())
}

impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let Ok(items) = self.items.lock() else {
            return String::new();
        };
        serde_json::to_string_pretty(&*items).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let next: Vec<Discovery> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut items = self
            .items
            .lock()
            .map_err(|_| "toolbox is unavailable".to_string())?;
        self.save(&next);
        *items = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_restores_into_a_fresh_store() {
        let a = Store::load(&temp_dir("snapshot"));
        a.share(1, "alpha", &input("FFmpeg", "cli")).unwrap();
        let b = Store::load(&temp_dir("snapshot-restored"));
        b.restore(&a.snapshot()).unwrap();
        assert_eq!(b.snapshot(), a.snapshot());
        assert_eq!(b.list().len(), 1);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("apid-toolbox-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn input(name: &str, kind: &str) -> Value {
        json!({
            "name": name,
            "kind": kind,
            "what_for": "extract audio from a video file",
            "how": "pip install ffmpeg-python; run ffmpeg -i in.mp4 out.wav",
            "verdict": "works on a 4 GB VM",
            "tags": ["ffmpeg-tools", " Media "],
        })
    }

    #[test]
    fn share_new_then_search_by_tag_and_word() {
        let store = Store::load(&temp_dir("new"));
        let d = store.share(1, "alpha", &input("FFmpeg", "cli")).unwrap();
        assert_eq!(d.id, "d1");
        assert_eq!(
            d.tags,
            vec!["ffmpeg-tools".to_string(), "media".to_string()]
        );
        assert_eq!(store.search("media").len(), 1, "found by tag");
        assert_eq!(
            store.search("extract").len(),
            1,
            "found by a word of what_for"
        );
        assert_eq!(
            store.search("extract zzz").len(),
            0,
            "every word must match"
        );
        assert_eq!(store.search("").len(), 1, "empty query lists all");
    }

    #[test]
    fn share_rules() {
        // same_agent_resharing_updates_in_place: resharing the same name by the
        // same agent updates the entry in place.
        {
            let store = Store::load(&temp_dir("same"));
            store.share(1, "alpha", &input("FFmpeg", "cli")).unwrap();
            let mut again = input("ffmpeg", "cli");
            again["verdict"] = json!("works, slow on large files");
            let d = store.share(1, "alpha", &again).unwrap();
            assert_eq!(d.id, "d1");
            assert_eq!(store.list().len(), 1);
            assert_eq!(
                store.search("slow")[0].verdict,
                "works, slow on large files"
            );
        }
        // another_agent_endorses_the_same_name: another agent sharing the same
        // name endorses the entry and appends its verdict.
        {
            let store = Store::load(&temp_dir("endorse"));
            store.share(1, "alpha", &input("FFmpeg", "cli")).unwrap();
            let mut beta = input("FFMPEG", "cli");
            beta["verdict"] = json!("also works, used it for podcasts");
            store.share(2, "beta", &beta).unwrap();
            let items = store.list();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].endorsed_by, vec![2]);
            assert!(items[0].verdict.contains("works on a 4 GB VM"));
            assert!(
                items[0]
                    .verdict
                    .contains("\n— beta: also works, used it for podcasts")
            );
        }
        // bad_kind_is_refused: an unknown kind is refused, names the field, and
        // stores nothing.
        {
            let store = Store::load(&temp_dir("kind"));
            let err = store
                .share(1, "alpha", &input("Dance", "dance"))
                .unwrap_err();
            assert!(err.contains("kind"), "error names the field: {err}");
            assert!(store.list().is_empty());
        }
    }

    #[test]
    fn owner_flags_unflags_and_removes() {
        let dir = temp_dir("owner");
        let store = Store::load(&dir);
        store.share(1, "alpha", &input("FFmpeg", "cli")).unwrap();
        let d = store.flag("d1", Some("  leaks keys  ".into())).unwrap();
        assert_eq!(
            d.flagged.as_deref(),
            Some("leaks keys"),
            "reason is trimmed"
        );
        assert_eq!(
            Store::load(&dir).list()[0].flagged.as_deref(),
            Some("leaks keys"),
            "flag persists"
        );
        assert_eq!(
            store.flag("d1", Some("   ".into())).unwrap().flagged,
            None,
            "a blank reason clears"
        );
        store.flag("d1", Some("again".into())).unwrap();
        assert_eq!(store.flag("d1", None).unwrap().flagged, None, "null clears");
        assert!(
            store.flag("d1", Some("x".repeat(REASON_MAX + 1))).is_err(),
            "over 500 is refused"
        );
        assert_eq!(store.flag("d9", None).unwrap_err(), "no discovery d9");
        assert_eq!(store.remove("d1").unwrap().name, "FFmpeg");
        assert!(store.list().is_empty(), "removed from the list");
        assert_eq!(Store::load(&dir).list().len(), 0, "removal persists");
        assert_eq!(store.remove("d1").unwrap_err(), "no discovery d1");
    }

    #[test]
    fn share_persists_across_loads() {
        let dir = temp_dir("persist");
        Store::load(&dir)
            .share(1, "alpha", &input("FFmpeg", "cli"))
            .unwrap();
        let reloaded = Store::load(&dir);
        let items = reloaded.list();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "FFmpeg");
        assert_eq!(reloaded.search("ffmpeg").len(), 1);
    }
}
