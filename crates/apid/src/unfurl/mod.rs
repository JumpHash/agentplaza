//! The unfurl (docs/design/workspace.md, section 3.3): card data for the references clients show as chips and hover
//! cards. `POST /v1/unfurl {refs}` answers many at once in order; `GET /v1/unfurl?url=` classifies one URL and answers
//! for it. Both are owner routes (hub_auth gates every `/v1` route that is not public).
//!
//! The hub fetches only from an allow-list: GitHub's API (with the owner's GitHub token when there is one, else
//! anonymous), and its own agents, tasks, library tools and deliverables. Workspace files are not unfurled here: clients
//! read them through the IDE routes. Remote answers are cached (open items 10 minutes, merged, closed and completed
//! items and commits an hour, failures a minute; see `cards::ttl_ms`) in `plaza.db` (`unfurl_cache`, one row per
//! reference, keyed by its canonical text). Agents, tasks and the hub's records are live state and are read on every
//! request.

mod cards;
#[cfg(test)]
mod tests;

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use apz_http::{Request, Response, RouteDoc, Router};
use apz_rich::{Ctx, Ref, Span};
use apz_store::rusqlite::Connection;
use apz_store::table::{Changes, Loaded, Row, Value as SqlValue};
use apz_store::{Db, RowSet, StoreError, Table, TableDef};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::App;
use crate::store::sql;

/// The most references one request may ask for.
pub const MAX_REFS: usize = 50;
/// How many references are fetched at once.
const CONCURRENCY: usize = 8;
const GITHUB_API: &str = "https://api.github.com";
/// The time limit of one GitHub read.
const GITHUB_TIMEOUT: Duration = Duration::from_secs(15);

/// Where remote answers come from. The live hub reads GitHub; tests answer from fixtures.
pub trait Source: Sync {
    /// A GET on the GitHub API (`path` starts with `/`): the status and the JSON body, or why the call could not be made.
    fn github(&self, path: &str) -> Result<(u16, Value), &'static str>;
}

/// One cached answer: a row of `unfurl_cache`, keyed by the reference's canonical text (see [`cache_key`]). The card is
/// what the provider answered, shown to people: content, never read back as state. How long the answer stays fresh is
/// decided when it is fetched (`ttl_ms`, from its kind and state), so the cache never reads the card.
#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "unfurl_cache")]
pub struct Answer {
    #[table(key)]
    pub key: String,
    pub ok: bool,
    #[table(content)]
    pub card: Option<Value>,
    pub error: Option<String>,
    pub fetched_ms: u64,
    pub ttl_ms: u64,
}

/// The table `unfurl_cache`: one row per reference, keyed by its canonical text.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Answer)
}

impl Answer {
    /// The row for `key`, from the answer `item` the route returns for the fetch, made at `now`.
    fn of(key: String, item: &Value, now: u64) -> Answer {
        Answer {
            key,
            ok: item["ok"].as_bool() == Some(true),
            card: item.get("card").cloned(),
            error: item["error"].as_str().map(str::to_string),
            fetched_ms: now,
            ttl_ms: cards::ttl_ms(item),
        }
    }

    /// The answer the route returns for this row: `{ok, card}`, or `{ok, error}`.
    fn answer_json(&self) -> Value {
        if self.ok {
            json!({"ok": true, "card": self.card})
        } else {
            json!({"ok": false, "error": self.error})
        }
    }
}

/// The hub's cache of remote answers, in `plaza.db`: one row per reference. Each read and write goes to the database;
/// there is no memory copy.
pub struct Cache {
    db: Arc<Db>,
}

impl Cache {
    /// A cache over the hub's `plaza.db`.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// The row stored for `key`, if any. A failed read is logged and answers a miss.
    fn get(&self, key: &str) -> Option<Answer> {
        match self.db.read(|conn| find(conn, key)) {
            Ok(found) => found,
            Err(e) => {
                eprintln!("apid: unfurl cache read failed: {e}");
                None
            }
        }
    }

    /// Stores `answer` as the row for its key, replacing the one stored, and waits for the commit. A failed write is
    /// logged.
    fn put(&self, answer: Answer) {
        if let Err(e) = self.db.write(move |conn| save(conn, &answer)) {
            eprintln!("apid: unfurl cache write failed: {e}");
        }
    }
}

/// Reads the row stored for `key`.
fn find(conn: &Connection, key: &str) -> Result<Option<Answer>, StoreError> {
    let def = def();
    let width = def.key.len() + def.columns.len();
    let mut stmt = conn.prepare_cached(sql::UNFURL_GET)?;
    let mut rows = stmt.query([key])?;
    let Some(r) = rows.next()? else {
        return Ok(None);
    };
    let mut values = Vec::with_capacity(width);
    for i in 0..width {
        values.push(r.get::<_, SqlValue>(i)?);
    }
    let columns = values.split_off(def.key.len());
    let row = Row {
        key: values,
        columns,
    };
    Answer::take(def, &row, &Loaded::default()).map(Some)
}

/// Writes `answer` as the row for its key, inserted or replaced.
fn save(conn: &Connection, answer: &Answer) -> Result<(), StoreError> {
    let def = def();
    let mut rows = RowSet::one(def, answer)?;
    let mut changes = Changes::default();
    changes.upserts = rows
        .tables
        .remove(&def.name)
        .unwrap_or_default()
        .into_iter()
        .map(|row| (def.name.clone(), row))
        .collect();
    apz_store::table::apply(conn, def, &changes)
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
    let cache = &app.unfurl;
    let mut out = Vec::with_capacity(refs.len());
    for chunk in refs.chunks(CONCURRENCY) {
        std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|r| {
                    let (source, cache) = (&source, cache);
                    scope.spawn(move || match local(app, r) {
                        Some(answer) => answer,
                        None => resolve_remote(source, cache, r, now),
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

/// The answer for a remote reference: from the cache while it is fresh, else fetched and cached (failures too). A
/// reference no provider answers has no cache key: it is answered, not stored.
fn resolve_remote(source: &dyn Source, cache: &Cache, r: &Ref, now: u64) -> Value {
    let Some(key) = cache_key(r) else {
        return answer_json(fetch(source, r));
    };
    if let Some(found) = cache.get(&key)
        && now.saturating_sub(found.fetched_ms) < found.ttl_ms
    {
        return found.answer_json();
    }
    let item = answer_json(fetch(source, r));
    cache.put(Answer::of(key, &item, now));
    item
}

/// The route's answer for a fetch: `{ok, card}`, or `{ok, error}` with the reason the card is missing.
fn answer_json(result: Result<Value, &'static str>) -> Value {
    match result {
        Ok(card) => json!({"ok": true, "card": card}),
        Err(error) => json!({"ok": false, "error": error}),
    }
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

/// The canonical text of a remote reference, the key its answer is cached under: one per answer, such as
/// `github_pull:o/r#12`, `github_file:o/r@main:src/lib.rs#L4` or `gist:abc123`. Only references `plan` accepts have
/// one (the names are checked, so none holds a separator); a path and a reference are percent-encoded, so no text in
/// them can read as a separator. None for a reference no provider answers.
pub fn cache_key(r: &Ref) -> Option<String> {
    plan(r)?;
    Some(match r {
        Ref::GithubPull {
            owner,
            repo,
            number,
        } => format!("github_pull:{owner}/{repo}#{number}"),
        Ref::GithubIssue {
            owner,
            repo,
            number,
        } => format!("github_issue:{owner}/{repo}#{number}"),
        Ref::GithubCommit { owner, repo, sha } => format!("github_commit:{owner}/{repo}@{sha}"),
        Ref::GithubRepo { owner, repo } => format!("github_repo:{owner}/{repo}"),
        Ref::GithubFile {
            owner,
            repo,
            reference,
            path,
            line,
            dir,
        } => {
            // A folder and a file are two answers (the card links to each as its own page).
            let kind = if *dir { "github_dir" } else { "github_file" };
            let anchor = line.map(|n| format!("#L{n}")).unwrap_or_default();
            format!(
                "{kind}:{owner}/{repo}@{}:{}{anchor}",
                seg(reference),
                pct(path, true)
            )
        }
        Ref::GithubRelease { owner, repo, tag } => format!("github_release:{owner}/{repo}@{tag}"),
        Ref::GithubRun { owner, repo, id } => format!("github_run:{owner}/{repo}#{id}"),
        Ref::Gist { id } => format!("gist:{id}"),
        _ => return None,
    })
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
        Ref::Agent { cid, .. } => agent_card(app, *cid),
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

fn agent_card(app: &App, cid: Option<u64>) -> Result<Value, &'static str> {
    let agent = cid.and_then(|cid| app.agents.get(cid)).ok_or("not_found")?;
    Ok(json!({
        "name": agent.label(),
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
        agents: Vec::new(),
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
