//! The unfurl (docs/design/workspace.md, section 3.3): card data for the references clients show as chips and hover
//! cards. `POST /v1/unfurl {refs}` answers many at once in order; `GET /v1/unfurl?url=` classifies one URL and answers
//! for it. Both are owner routes (hub_auth gates every `/v1` route that is not public).
//!
//! The hub fetches only from an allow-list: GitHub's API (with the owner's GitHub token when there is one, else
//! anonymous), and its own agents, tasks, library tools and deliverables. Workspace files are not unfurled here: clients read them through the IDE routes. Remote answers are
//! cached (open items 10 minutes, merged, closed and completed items and commits an hour, failures a minute; see
//! `cards::ttl_ms`), in Postgres (`unfurl_cache`) or, without Postgres, in memory. Agents, tasks and the hub's records
//! are live state and are read on every request.

mod cards;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use apz_http::{Request, Response, RouteDoc, Router};
use apz_rich::{AgentName, Ctx, Ref, Span};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::App;

/// The most references one request may ask for.
pub const MAX_REFS: usize = 50;
/// How many references are fetched at once.
const CONCURRENCY: usize = 8;
const GITHUB_API: &str = "https://api.github.com";
/// The time limit of one GitHub read.
const GITHUB_TIMEOUT: Duration = Duration::from_secs(15);
/// The most answers the memory copy keeps.
const MEMORY_LIMIT: usize = 4096;

/// Where remote answers come from. The live hub reads GitHub; tests answer from fixtures.
pub trait Source: Sync {
    /// A GET on the GitHub API (`path` starts with `/`): the status and the JSON body, or why the call could not be made.
    fn github(&self, path: &str) -> Result<(u16, Value), &'static str>;
}

/// One stored answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The answer as the route returns it: `{ok, card}` or `{ok, error}`.
    pub data: Value,
    pub ok: bool,
    pub fetched_ms: u64,
}

/// Where remote answers are kept between requests.
pub trait Store: Sync {
    fn get(&self, key: &str) -> Option<Row>;
    fn put(&self, key: &str, row: Row);
}

/// The memory copy of the cache, for a hub without Postgres. Bounded: a full map drops its stalest answer.
#[derive(Default)]
pub struct Cache {
    rows: Mutex<HashMap<String, Row>>,
}

impl Store for Cache {
    fn get(&self, key: &str) -> Option<Row> {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    fn put(&self, key: &str, row: Row) {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        if rows.len() >= MEMORY_LIMIT
            && !rows.contains_key(key)
            && let Some(stalest) = rows
                .iter()
                .min_by_key(|(_, r)| r.fetched_ms)
                .map(|(k, _)| k.clone())
        {
            rows.remove(&stalest);
        }
        rows.insert(key.to_string(), row);
    }
}

/// The hub's cache: Postgres when the hub has it, the memory copy otherwise.
struct Backed<'a>(&'a App);

impl Store for Backed<'_> {
    fn get(&self, key: &str) -> Option<Row> {
        if !self.0.db.enabled() {
            return self.0.unfurl.get(key);
        }
        match self.0.db.unfurl_get(key) {
            Ok(found) => found.map(|(data, ok, fetched_ms)| Row {
                data,
                ok,
                fetched_ms,
            }),
            Err(e) => {
                eprintln!("apid: unfurl cache read failed: {e}");
                None
            }
        }
    }

    fn put(&self, key: &str, row: Row) {
        if !self.0.db.enabled() {
            return self.0.unfurl.put(key, row);
        }
        if let Err(e) = self.0.db.unfurl_put(key, &row.data, row.ok, row.fetched_ms) {
            eprintln!("apid: unfurl cache write failed: {e}");
        }
    }
}

/// The hub's provider: the GitHub API with the owner's token.
struct Live<'a>(&'a App);

impl Source for Live<'_> {
    fn github(&self, path: &str) -> Result<(u16, Value), &'static str> {
        let mut headers = vec![
            ("User-Agent", "agentplaza".to_string()),
            ("Accept", "application/vnd.github+json".to_string()),
            ("X-GitHub-Api-Version", "2022-11-28".to_string()),
        ];
        if let Some(token) = crate::skills::import::github_token(self.0) {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        let url = format!("{GITHUB_API}{path}");
        let (status, bytes) =
            get_within(&url, &headers, GITHUB_TIMEOUT).map_err(|_| "unavailable")?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }
}

/// One GET with a time limit: the status and the body bytes of the reply.
fn get_within(
    url: &str,
    headers: &[(&str, String)],
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let mut builder = ureq::http::Request::builder().method("GET").uri(url);
    for (key, value) in headers {
        builder = builder.header(*key, value.as_str());
    }
    let request = builder
        .body(Vec::new())
        .map_err(|e| format!("bad request: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    let response = ureq::Agent::new_with_config(config)
        .run(request)
        .map_err(|e| format!("no answer: {e}"))?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading the answer: {e}"))?;
    Ok((status, bytes))
}

/// The answers for `refs`, in order. Agents, tasks and the hub's own records are read live; remote references come
/// from the cache when it is fresh, else from their provider.
pub fn unfurl(app: &App, refs: &[Ref], now: u64) -> Vec<Value> {
    let source = Live(app);
    let store = Backed(app);
    let mut out = Vec::with_capacity(refs.len());
    for chunk in refs.chunks(CONCURRENCY) {
        std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|r| {
                    let (source, store) = (&source, &store);
                    scope.spawn(move || match local(app, r) {
                        Some(answer) => answer,
                        None => resolve_remote(source, store, r, now),
                    })
                })
                .collect();
            for handle in handles {
                out.push(
                    handle
                        .join()
                        .unwrap_or_else(|_| json!({"ok": false, "error": "unavailable"})),
                );
            }
        });
    }
    out
}

/// The answer for a remote reference: from the store while it is fresh, else fetched and stored (failures too).
fn resolve_remote(source: &dyn Source, store: &dyn Store, r: &Ref, now: u64) -> Value {
    let key = serde_json::to_string(r).unwrap_or_default();
    if let Some(row) = store.get(&key)
        && now.saturating_sub(row.fetched_ms) < cards::ttl_ms(&row.data)
    {
        return row.data;
    }
    let item = match fetch(source, r) {
        Ok(card) => json!({"ok": true, "card": card}),
        Err(error) => json!({"ok": false, "error": error}),
    };
    let ok = item["ok"].as_bool() == Some(true);
    store.put(
        &key,
        Row {
            data: item.clone(),
            ok,
            fetched_ms: now,
        },
    );
    item
}

/// Calls the provider for `r` and builds its card. The error is the reason the card is missing.
fn fetch(source: &dyn Source, r: &Ref) -> Result<Value, &'static str> {
    let path = plan(r).ok_or("unsupported")?;
    let (status, body) = source.github(&path)?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &body));
    }
    Ok(cards::card(r, &body))
}

/// The error for a failed provider answer: a rate limit (GitHub answers 403 with a "rate limit" message, or 429), a
/// missing item, or anything else.
fn status_error(status: u16, body: &Value) -> &'static str {
    let limited = status == 429
        || (status == 403
            && body["message"]
                .as_str()
                .is_some_and(|m| m.to_ascii_lowercase().contains("rate limit")));
    if limited {
        "rate_limited"
    } else if status == 404 {
        "not_found"
    } else {
        "unavailable"
    }
}

/// The GitHub API path for a remote reference, with every name it puts in the path checked: the Ref may come from a
/// client's JSON, and the call may carry the owner's token. None for a reference with no provider.
fn plan(r: &Ref) -> Option<String> {
    let path = match r {
        Ref::GithubPull {
            owner,
            repo,
            number,
        } => format!("{}/pulls/{number}", repo_path(owner, repo)?),
        Ref::GithubIssue {
            owner,
            repo,
            number,
        } => format!("{}/issues/{number}", repo_path(owner, repo)?),
        Ref::GithubCommit { owner, repo, sha } => {
            format!("{}/commits/{}", repo_path(owner, repo)?, sha_of(sha)?)
        }
        Ref::GithubRepo { owner, repo } => repo_path(owner, repo)?,
        Ref::GithubFile {
            owner,
            repo,
            reference,
            path,
            ..
        } => contents_path(owner, repo, reference, path)?,
        Ref::GithubRelease { owner, repo, tag } => format!(
            "{}/releases/tags/{}",
            repo_path(owner, repo)?,
            name_of_tag(tag)?
        ),
        Ref::GithubRun { owner, repo, id } => {
            format!("{}/actions/runs/{id}", repo_path(owner, repo)?)
        }
        Ref::Gist { id } => format!("/gists/{}", name_of(id)?),
        _ => return None,
    };
    Some(path)
}

fn repo_path(owner: &str, repo: &str) -> Option<String> {
    (name_ok(owner) && name_ok(repo)).then(|| format!("/repos/{owner}/{repo}"))
}

/// `/repos/o/r/contents/<path>?ref=<ref>` for a file or folder; every segment of the path checked.
fn contents_path(owner: &str, repo: &str, reference: &str, path: &str) -> Option<String> {
    let base = repo_path(owner, repo)?;
    if reference.is_empty() {
        return None;
    }
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| !segment_ok(s)) {
        return None;
    }
    let joined = segments.join("/");
    let tail = if joined.is_empty() {
        String::new()
    } else {
        format!("/{}", pct(&joined, true))
    };
    Some(format!("{base}/contents{tail}?ref={}", seg(reference)))
}

fn sha_of(sha: &str) -> Option<&str> {
    (sha.len() >= 7 && sha.len() <= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then_some(sha)
}

fn name_of(text: &str) -> Option<&str> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(text)
}

fn name_of_tag(tag: &str) -> Option<String> {
    (!tag.is_empty() && !tag.contains('/')).then(|| seg(tag))
}

/// A GitHub owner or repository: letters, digits, `-`, `_` and `.`; not `.` or `..`.
fn name_ok(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A segment of a file path: any characters but a separator or control character, and not `.` or `..` (the
/// characters are percent-encoded on the way out).
fn segment_ok(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && !text.chars().any(|c| c == '/' || c.is_control())
}

/// The answer for a reference the hub reads itself: agents, tasks, deliverables and library tools. None for the rest.
fn local(app: &App, r: &Ref) -> Option<Value> {
    let found = match r {
        Ref::Agent { name, cid } => agent_card(app, name.as_deref(), *cid),
        Ref::Task { number } => task_card(app, *number),
        Ref::Library { id } => deliverable_card(app, id),
        Ref::LibraryTool { name, version } => tool_card(app, name, *version),
        _ => return None,
    };
    Some(match found {
        Ok(mut card) => {
            card["kind"] = json!(cards::kind_of(r));
            json!({"ok": true, "card": card})
        }
        Err(error) => json!({"ok": false, "error": error}),
    })
}

fn agent_card(app: &App, name: Option<&str>, cid: Option<u64>) -> Result<Value, &'static str> {
    let agent = match (name, cid) {
        (Some(name), _) => app.agents.list().into_iter().find(|a| a.name == name),
        (None, Some(cid)) => app.agents.get(cid),
        (None, None) => None,
    }
    .ok_or("not_found")?;
    Ok(json!({
        "name": agent.name,
        "cid": agent.cid,
        "role": agent.role,
        "model": agent.model_override,
        "status": agent.status,
        "task": active_task(app, agent.cid),
        "last_active_ms": agent.last_seen_ms,
    }))
}

/// The task the agent holds (claimed or awaiting review), as `{number, title, state}`; null when it holds none.
fn active_task(app: &App, cid: u64) -> Value {
    app.plaza
        .tasks
        .list()
        .into_iter()
        .find_map(|task| {
            let view = task.view();
            let held =
                view.agent_cid == Some(cid) && matches!(view.state, "claimed" | "awaiting_review");
            held.then(
                || json!({"number": number_of(view.id), "title": view.goal, "state": view.state}),
            )
        })
        .unwrap_or(Value::Null)
}

/// `t528` is task 528.
fn number_of(id: &str) -> Option<u64> {
    id.strip_prefix('t')?.parse().ok()
}

fn task_card(app: &App, number: u64) -> Result<Value, &'static str> {
    let id = format!("t{number}");
    let task = app.plaza.tasks.find(&id).ok_or("not_found")?;
    let view = task.view();
    let project = app
        .projects
        .list()
        .into_iter()
        .find(|p| p.tasks.contains(&id))
        .map(|p| p.id);
    Ok(json!({
        "number": number,
        "title": view.goal,
        "status": view.state,
        "assignee": view.assignee_id,
        "project": project,
        "budget": view.budget,
        "updated_ms": view.updated_at,
    }))
}

fn deliverable_card(app: &App, id: &str) -> Result<Value, &'static str> {
    let item = app.deliverables.find(id).ok_or("not_found")?;
    Ok(json!({
        "id": item.id,
        "name": item.name,
        "title": item.title,
        "content_type": item.content_type,
        "size": item.size,
        "author": item.author,
        "task": item.task,
        "created_ms": item.created_ms,
        "source": item.kind,
        "audience": item.audience,
    }))
}

fn tool_card(app: &App, name: &str, version: u32) -> Result<Value, &'static str> {
    let entry = app
        .library
        .all()
        .into_iter()
        .find(|e| e.name == name && e.version == version)
        .ok_or("not_found")?;
    Ok(json!({
        "name": entry.name,
        "version": entry.version,
        "author": entry.author,
        "description": entry.description,
        "state": entry.state,
        "files": entry.files.len(),
        "created_ms": entry.created_ms,
    }))
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

/// The classifier's context on this hub: the hub's own host and the agent names.
fn server_ctx(app: &App) -> Ctx {
    Ctx {
        hub_host: Some(app.config.public_url.clone()),
        agents: app
            .agents
            .list()
            .into_iter()
            .filter(|a| !a.name.is_empty())
            .map(|a| AgentName {
                name: a.name,
                cid: a.cid,
            })
            .collect(),
        ..Ctx::default()
    }
}

#[derive(Debug, Deserialize)]
struct Batch {
    refs: Vec<Ref>,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/unfurl", |app, req, _| batch_route(app, req))
        .doc(RouteDoc::new("Get cards for a batch of references").outbound())
        .get("/v1/unfurl", |app, req, _| one_route(app, req))
        .doc(
            RouteDoc::new("Get the card for one URL")
                .outbound()
                .errors(&[422]),
        )
}

fn batch_route(app: &App, req: &Request) -> Response {
    let batch = match req.json::<Batch>() {
        Ok(batch) => batch,
        Err(_) => {
            return Response::error(
                400,
                "the body is not {\"refs\": [...]} with known reference kinds",
            );
        }
    };
    if batch.refs.len() > MAX_REFS {
        return Response::error(400, "at most 50 references per request");
    }
    Response::json(200, &json!({ "cards": unfurl(app, &batch.refs, now()) }))
}

fn one_route(app: &App, req: &Request) -> Response {
    let Some(raw) = req.query_param("url") else {
        return Response::error(400, "the url query is missing");
    };
    let url = unescape(raw);
    let found = apz_rich::classify(&url, &server_ctx(app))
        .into_iter()
        .find_map(|span| match span {
            Span::Ref { r, .. } => Some(r),
            _ => None,
        });
    match found {
        Some(r) => Response::json(200, &unfurl(app, &[r], now()).remove(0)),
        None => Response::error(422, "not a reference the hub unfurls"),
    }
}

/// Percent-encodes for a URL; `/` stays when `slash` is set.
fn pct(raw: &str, slash: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (slash && b == b'/')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One percent-encoded path segment.
fn seg(raw: &str) -> String {
    pct(raw, false)
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// A query value decoded: `%XX` escapes and `+` for a space.
fn unescape(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (
            b[i],
            b.get(i + 1).and_then(|c| hex(*c)),
            b.get(i + 2).and_then(|c| hex(*c)),
        ) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
            (b'+', _, _) => out.push(b' '),
            (c, _, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
