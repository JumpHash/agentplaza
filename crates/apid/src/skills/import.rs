//! See docs/design/skill-library.md.
//! Import (a zip or a GitHub location, previewed then committed), GitHub update checks, and zip export
//! (docs/design/skill-library.md, "Import, export, GitHub").
//!
//! Nothing here runs a skill. The previews wait in memory for 15 minutes; a commit creates the picked skills with
//! `Skills::prepare`, so the same checks and size limits apply as to any other version.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use apz_http::{Response, RouteDoc, Router};
use apz_skill::archive::{
    Entry, FoundSkill, Limits, find_skills, parse_github_url, read_zip, write_zip,
};
use apz_skill::{FileInfo, check};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::events::Bus;
use crate::skills::{
    FileBody, FileIn, MAX_TITLE, NewSkill, Skill, SkillStore, Skills, Source, StoreError,
    valid_binding, valid_id,
};
use crate::state::App;
use crate::vault::OnBehalf;

/// The largest zip an upload or a GitHub download may be.
pub const MAX_UPLOAD: usize = 100 * 1024 * 1024;
const PREVIEW_TTL_MS: u64 = 15 * 60 * 1000;
const MAX_PREVIEWS: usize = 16;
const MAX_EXPORT_IDS: usize = 100;
/// The vault secret that holds a GitHub token, and the consumer binding the owner bound it to.
const GITHUB_TOKEN: &str = "github_token";
const GITHUB_BINDING: &str = "github";
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// A commit id (the `application/vnd.github.sha` answer) is 40 bytes; a little more is refused.
const API_LIMIT: u64 = 256;
const OWNER: &str = "owner";
const GITHUB_ACTOR: &str = "github";

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/skills/import/preview", |app, req, _| {
            respond(zip_preview(&app.skills, previews(), &req.body, now()))
        })
        .doc(RouteDoc::new("Preview the skills in an uploaded zip").errors(&[413, 422]))
        .post("/v1/skills/github/preview", |app, req, _| {
            match req.json::<UrlBody>() {
                Ok(body) => respond(github_preview(
                    &app.skills,
                    previews(),
                    &Net,
                    github_token(app).as_deref(),
                    &body.url,
                    now(),
                )),
                Err(_) => Response::error(400, "the body is not {\"url\": \"...\"}"),
            }
        })
        .doc(
            RouteDoc::new("Preview the skills in a GitHub folder")
                .outbound()
                .errors(&[400, 422, 502]),
        )
        .post("/v1/skills/import/commit", |app, req, _| {
            match req.json::<CommitBody>() {
                Ok(body) => respond(commit(&app.skills, &app.bus, previews(), body, now())),
                Err(_) => Response::error(400, "the body is not valid JSON for this route"),
            }
        })
        .doc(RouteDoc::new("Create the skills picked from a preview").errors(&[400, 404, 409, 422]))
        .post("/v1/skills/{id}/github/check", |app, _, p| {
            respond(github_check(
                &app.skills,
                &Net,
                github_token(app).as_deref(),
                p.get("id"),
            ))
        })
        .doc(
            RouteDoc::new("Check whether a GitHub skill has a newer commit")
                .outbound()
                .errors(&[404, 422, 502]),
        )
        .post("/v1/skills/{id}/github/update", |app, _, p| {
            respond(github_update(
                &app.skills,
                &app.bus,
                &Net,
                github_token(app).as_deref(),
                p.get("id"),
                now(),
            ))
        })
        .doc(
            RouteDoc::new("Update a GitHub skill to its latest commit")
                .outbound()
                .errors(&[404, 422, 502]),
        )
        .get("/v1/skills/export", |app, req, _| {
            respond_zip(export_many(
                &app.skills,
                req.query_param("ids").unwrap_or(""),
            ))
        })
        .doc(RouteDoc::new("Export several skills as one zip").errors(&[404, 422]))
        .get("/v1/skills/{id}/versions/{v}/export", |app, _, p| {
            respond_zip(export_version(&app.skills, p.get("id"), p.get("v")))
        })
        .doc(RouteDoc::new("Export one version of a skill as a zip").errors(&[404]))
}

#[derive(Debug, Deserialize)]
struct UrlBody {
    url: String,
}

#[derive(Debug, Deserialize)]
struct CommitBody {
    preview_id: String,
    picks: Vec<String>,
    #[serde(default)]
    bindings: Vec<String>,
}

/// A refusal or a failure, with its status.
#[derive(Debug)]
pub enum Fail {
    Store(StoreError),
    Status(u16, String),
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Fail {
        Fail::Store(e)
    }
}

impl From<String> for Fail {
    fn from(e: String) -> Fail {
        Fail::Store(StoreError::Failed(e))
    }
}

fn fail(f: Fail) -> Response {
    match f {
        Fail::Status(code, msg) => Response::error(code, &msg),
        Fail::Store(StoreError::NotFound) => Response::error(404, "no such skill"),
        Fail::Store(StoreError::Conflict(msg)) => Response::error(409, &msg),
        Fail::Store(StoreError::Invalid(msg)) => Response::error(422, &msg),
        Fail::Store(StoreError::Failed(msg)) => Response::error(500, &msg),
    }
}

fn respond(result: Result<Value, Fail>) -> Response {
    match result {
        Ok(value) => Response::json(200, &value),
        Err(f) => fail(f),
    }
}

fn respond_zip(result: Result<(Vec<u8>, String), Fail>) -> Response {
    match result {
        Ok((bytes, name)) => Response::new(200, "application/zip", bytes).with_header(
            "content-disposition",
            &format!("attachment; filename=\"{name}\""),
        ),
        Err(f) => fail(f),
    }
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

/// The owner's GitHub token: the vault's `github_token` bound to the `github` consumer, else GITHUB_TOKEN.
pub(crate) fn github_token(app: &App) -> Option<String> {
    app.vault
        .value_for(GITHUB_TOKEN, GITHUB_BINDING, &OnBehalf::Hub)
}

// ---- previews ----

/// A found set of skills, held until its `expires_ms`.
#[derive(Clone)]
pub struct Preview {
    pub source: Source,
    pub skills: Vec<FoundSkill>,
    pub expires_ms: u64,
}

/// The previews in memory: at most 16, each for 15 minutes.
#[derive(Default)]
pub struct Previews {
    inner: Mutex<HashMap<String, Preview>>,
}

impl Previews {
    pub fn new() -> Previews {
        Previews::default()
    }

    /// Stores `preview` and returns its random id. Expired previews go first; when 16 are held the oldest goes.
    pub fn put(&self, preview: Preview, now: u64) -> String {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, p| p.expires_ms > now);
        while map.len() >= MAX_PREVIEWS {
            let oldest = map
                .iter()
                .min_by_key(|(_, p)| p.expires_ms)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    map.remove(&k);
                }
                None => break,
            }
        }
        let id = loop {
            let candidate = new_id();
            if !map.contains_key(&candidate) {
                break candidate;
            }
        };
        map.insert(id.clone(), preview);
        id
    }

    /// The preview, unless it is unknown or expired at `now`.
    pub fn get(&self, id: &str, now: u64) -> Option<Preview> {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.get(id).filter(|p| p.expires_ms > now).cloned()
    }
}

fn previews() -> &'static Previews {
    static PREVIEWS: OnceLock<Previews> = OnceLock::new();
    PREVIEWS.get_or_init(Previews::new)
}

/// A 32-hex-digit id from std's per-process random hash keys (preview ids are owner-only and short-lived).
fn new_id() -> String {
    let mut id = String::with_capacity(32);
    for _ in 0..2 {
        id.push_str(&format!(
            "{:016x}",
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        ));
    }
    id
}

/// The preview answer: its id, the source, when it expires, and per skill its path (the folder), name, title,
/// report, file count and the live skill of the same name (`exists`).
fn offer(
    skills: &Skills,
    previews: &Previews,
    source: Source,
    found: Vec<FoundSkill>,
    now: u64,
) -> Result<Value, Fail> {
    let live = skills.store().list(false)?;
    let mut items = Vec::with_capacity(found.len());
    for f in &found {
        let report = check(&f.skill_md, &infos(&f.files));
        let title = title_of(&f.dir, report.name.as_deref());
        let exists = report
            .name
            .as_deref()
            .and_then(|n| live.iter().find(|s| s.name == n))
            .map(|s| s.id.clone());
        items.push(json!({
            "path": f.dir,
            "name": report.name,
            "title": title,
            "report": report,
            "files": f.files.len(),
            "exists": exists,
        }));
    }
    let expires_ms = now + PREVIEW_TTL_MS;
    let preview_id = previews.put(
        Preview {
            source: source.clone(),
            skills: found,
            expires_ms,
        },
        now,
    );
    Ok(
        json!({"preview_id": preview_id, "source": source, "expires_ms": expires_ms, "skills": items}),
    )
}

/// A zip upload: every folder under the archive root with a SKILL.md.
fn zip_preview(
    skills: &Skills,
    previews: &Previews,
    bytes: &[u8],
    now: u64,
) -> Result<Value, Fail> {
    if bytes.len() > MAX_UPLOAD {
        return Err(Fail::Status(
            413,
            "the archive is larger than 100 MB".into(),
        ));
    }
    let entries = read_zip(bytes, Limits::ARCHIVE).map_err(|e| Fail::Status(422, e))?;
    let found = find_skills(&entries, "");
    if found.is_empty() {
        return Err(Fail::Status(422, "no SKILL.md in this archive".into()));
    }
    offer(skills, previews, Source::Upload, found, now)
}

/// A GitHub location: the ref resolved to a commit, that commit's zipball, the skills under the requested path.
fn github_preview(
    skills: &Skills,
    previews: &Previews,
    fetch: &dyn Fetch,
    token: Option<&str>,
    url: &str,
    now: u64,
) -> Result<Value, Fail> {
    let gh = parse_github_url(url).map_err(|e| Fail::Status(422, e))?;
    let commit = resolve_commit(fetch, token, &gh.owner, &gh.repo, gh.git_ref.as_deref())?;
    let entries = download(fetch, token, &gh.owner, &gh.repo, &commit)?;
    let found = repo_relative(find_skills(&entries, &gh.path));
    if found.is_empty() {
        return Err(Fail::Status(
            422,
            "no SKILL.md under that path in this repository".into(),
        ));
    }
    let source = Source::Github {
        repo: format!("{}/{}", gh.owner, gh.repo),
        git_ref: gh.git_ref.clone(),
        path: gh.path.clone(),
        commit,
    };
    offer(skills, previews, source, found, now)
}

// ---- commit ----

/// Creates the picked skills, all or none: every pick is checked and its name is free before anything is written.
fn commit(
    skills: &Skills,
    bus: &Bus,
    previews: &Previews,
    body: CommitBody,
    now: u64,
) -> Result<Value, Fail> {
    let preview = previews
        .get(&body.preview_id, now)
        .ok_or_else(|| Fail::Status(404, "no such preview, or it has expired".into()))?;
    if body.picks.is_empty() {
        return Err(Fail::Status(422, "pick at least one skill".into()));
    }
    let mut bindings = body.bindings.clone();
    if let Some(bad) = bindings.iter().find(|b| !valid_binding(b)) {
        return Err(Fail::Status(
            422,
            format!("{bad} is not a binding (plaza or role:<role>)"),
        ));
    }
    bindings.sort();
    bindings.dedup();

    let mut seen = HashSet::new();
    let mut picked = Vec::new();
    for dir in &body.picks {
        if !seen.insert(dir.as_str()) {
            continue;
        }
        let found = preview
            .skills
            .iter()
            .find(|f| &f.dir == dir)
            .ok_or_else(|| Fail::Status(422, format!("{dir} is not a skill in this preview")))?;
        picked.push(found);
    }

    let live = skills.store().list(false)?;
    let origin = origin_text(&preview.source);
    let commit_sha = match &preview.source {
        Source::Github { commit, .. } => Some(commit.clone()),
        _ => None,
    };
    let mut names = HashSet::new();
    let mut prepared = Vec::with_capacity(picked.len());
    for found in picked {
        let report = check(&found.skill_md, &infos(&found.files));
        if let Some(name) = &report.name {
            if let Some(holder) = live.iter().find(|s| &s.name == name) {
                return Err(Fail::Store(StoreError::Conflict(format!(
                    "{name} is already a live skill ({})",
                    holder.id
                ))));
            }
            if !names.insert(name.clone()) {
                return Err(Fail::Store(StoreError::Conflict(format!(
                    "{name} is in the picks twice"
                ))));
            }
        }
        let title = title_of(&found.dir, report.name.as_deref());
        let nv = skills.prepare(
            found.skill_md.clone(),
            files_in(&found.files),
            format!("Imported from {origin}"),
            commit_sha.clone(),
        )?;
        prepared.push((title, nv, source_for(&preview.source, &found.dir)));
    }

    let mut out = Vec::with_capacity(prepared.len());
    for (title, nv, source) in prepared {
        let id = unique_id(skills.store(), &title)?;
        let new = NewSkill {
            id,
            title,
            tags: Vec::new(),
            bindings: bindings.clone(),
            source,
        };
        let skill = skills.store().create(&new, &nv, OWNER, now)?;
        bus.publish("skill", &json!({ "id": skill.id, "action": "import" }));
        out.push(skills.summary(&skill.id)?);
    }
    Ok(json!({"skills": out}))
}

/// The slug of `title`, with -2, -3 and so on while taken.
fn unique_id(store: &dyn SkillStore, title: &str) -> Result<String, Fail> {
    let base: String = apz_prompt::slug(title).chars().take(50).collect();
    let base = base.trim_end_matches('-').to_string();
    let base = if base.is_empty() {
        "skill".to_string()
    } else {
        base
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while store.get(&candidate)?.is_some() {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    if !valid_id(&candidate) {
        return Err(Fail::Status(
            422,
            format!("{candidate} is not a valid skill id"),
        ));
    }
    Ok(candidate)
}

// ---- GitHub check and update ----

/// Whether the skill's ref now points at a newer commit than the one it was taken from.
fn github_check(
    skills: &Skills,
    fetch: &dyn Fetch,
    token: Option<&str>,
    id: &str,
) -> Result<Value, Fail> {
    let skill = skills.store().get(id)?.ok_or(StoreError::NotFound)?;
    let Source::Github {
        repo,
        git_ref,
        commit,
        ..
    } = &skill.source
    else {
        return Err(Fail::Status(
            422,
            "this skill was not imported from GitHub".into(),
        ));
    };
    let (owner, name) = split_repo(repo)?;
    let latest = resolve_commit(fetch, token, owner, name, git_ref.as_deref())?;
    Ok(json!({"current": commit, "latest": latest, "update_available": latest != *commit}))
}

/// Imports the skill at its path in the latest commit as a new version (author `github`). Unchanged: created false.
fn github_update(
    skills: &Skills,
    bus: &Bus,
    fetch: &dyn Fetch,
    token: Option<&str>,
    id: &str,
    now: u64,
) -> Result<Value, Fail> {
    let store = skills.store();
    let skill = store.get(id)?.ok_or(StoreError::NotFound)?;
    let Source::Github {
        repo,
        git_ref,
        path,
        commit,
    } = &skill.source
    else {
        return Err(Fail::Status(
            422,
            "this skill was not imported from GitHub".into(),
        ));
    };
    let (owner, name) = split_repo(repo)?;
    let latest = resolve_commit(fetch, token, owner, name, git_ref.as_deref())?;
    if latest == *commit {
        let last = store
            .versions(id)?
            .into_iter()
            .last()
            .ok_or(StoreError::NotFound)?;
        return Ok(json!({"version": last, "created": false}));
    }
    let entries = download(fetch, token, owner, name, &latest)?;
    let found = repo_relative(find_skills(&entries, path))
        .into_iter()
        .find(|f| &f.dir == path)
        .ok_or_else(|| Fail::Status(422, format!("{path} is not a skill in {}", short(&latest))))?;
    let nv = skills.prepare(
        found.skill_md.clone(),
        files_in(&found.files),
        format!("Updated from {repo}@{}", short(&latest)),
        Some(latest.clone()),
    )?;
    let meta = store.add_version(id, skill.latest_version, &nv, GITHUB_ACTOR, now)?;
    bus.publish("skill", &json!({ "id": id, "action": "import" }));
    Ok(json!({"version": meta, "created": true}))
}

/// The commit a ref points at: the API answers the bare sha with the `sha` media type.
fn resolve_commit(
    fetch: &dyn Fetch,
    token: Option<&str>,
    owner: &str,
    repo: &str,
    git_ref: Option<&str>,
) -> Result<String, Fail> {
    let url = format!(
        "https://api.github.com/repos/{owner}/{repo}/commits/{}",
        git_ref.unwrap_or("HEAD")
    );
    let body = fetch
        .get(&url, token, Some("application/vnd.github.sha"), API_LIMIT)
        .map_err(|e| Fail::Status(502, format!("GitHub did not resolve the ref: {e}")))?;
    let sha = String::from_utf8(body)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let well_formed = sha.len() == 40
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !well_formed {
        return Err(Fail::Status(
            502,
            "GitHub did not answer with a commit id".into(),
        ));
    }
    Ok(sha)
}

/// The commit's zipball, read within the archive limits. Its paths keep the `<repo>-<sha>/` folder: `find_skills`
/// needs that common top to match a path inside the repo; `repo_relative` removes it from the results.
fn download(
    fetch: &dyn Fetch,
    token: Option<&str>,
    owner: &str,
    repo: &str,
    sha: &str,
) -> Result<Vec<Entry>, Fail> {
    let url = format!("https://codeload.github.com/{owner}/{repo}/zip/{sha}");
    let bytes = fetch
        .get(&url, token, None, MAX_UPLOAD as u64)
        .map_err(|e| Fail::Status(502, format!("GitHub did not send the archive: {e}")))?;
    read_zip(&bytes, Limits::ARCHIVE)
        .map_err(|e| Fail::Status(422, format!("the GitHub archive: {e}")))
}

/// The found skills with the zipball's `<repo>-<sha>/` folder removed from each dir, so paths are relative to the
/// repository root (stable across commits).
fn repo_relative(found: Vec<FoundSkill>) -> Vec<FoundSkill> {
    found
        .into_iter()
        .map(|mut f| {
            f.dir = f
                .dir
                .split_once('/')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_default();
            f
        })
        .collect()
}

fn split_repo(repo: &str) -> Result<(&str, &str), Fail> {
    repo.split_once('/')
        .ok_or_else(|| Fail::Status(422, format!("{repo} is not owner/repo")))
}

// ---- export ----

/// One version as a zip whose folder is the skill's name.
fn export_version(skills: &Skills, id: &str, version: &str) -> Result<(Vec<u8>, String), Fail> {
    let v: u32 = version
        .parse()
        .map_err(|_| Fail::Status(404, "no such version".into()))?;
    let skill = skills.store().get(id)?.ok_or(StoreError::NotFound)?;
    let found = found_for(skills, &skill, v)?;
    let bytes = write_zip(&[found]).map_err(|e| Fail::Status(500, e))?;
    Ok((bytes, format!("{}.zip", file_stem(&skill))))
}

/// The live version (active, else latest) of each listed skill, in one zip.
fn export_many(skills: &Skills, ids: &str) -> Result<(Vec<u8>, String), Fail> {
    let ids: Vec<&str> = ids.split(',').filter(|s| !s.is_empty()).collect();
    if ids.is_empty() {
        return Err(Fail::Status(422, "ids is empty".into()));
    }
    if ids.len() > MAX_EXPORT_IDS {
        return Err(Fail::Status(
            422,
            format!("at most {MAX_EXPORT_IDS} skills in one export"),
        ));
    }
    let mut seen = HashSet::new();
    let mut picked = Vec::new();
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        let skill = skills.store().get(id)?.ok_or(StoreError::NotFound)?;
        if skill.archived_ms.is_some() {
            return Err(Fail::Status(422, format!("{id} is archived")));
        }
        let version = skill.active_version.unwrap_or(skill.latest_version);
        picked.push(found_for(skills, &skill, version)?);
    }
    let bytes = write_zip(&picked).map_err(|e| Fail::Status(500, e))?;
    Ok((bytes, "skills.zip".to_string()))
}

fn found_for(skills: &Skills, skill: &Skill, version: u32) -> Result<FoundSkill, Fail> {
    let v = skills
        .store()
        .version(&skill.id, version)?
        .ok_or(StoreError::NotFound)?;
    let files = v
        .files
        .iter()
        .map(|f| -> Result<Entry, String> {
            Ok(Entry {
                path: f.path.clone(),
                bytes: skills.blobs().get(&f.hash)?,
                executable: f.executable,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(FoundSkill {
        dir: skill.name.clone(),
        skill_md: v.skill_md,
        files,
    })
}

/// The skill's name as a file name when it is plain, else its id.
fn file_stem(skill: &Skill) -> String {
    let plain = !skill.name.is_empty()
        && skill
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if plain {
        skill.name.clone()
    } else {
        skill.id.clone()
    }
}

// ---- helpers ----

/// The HTTP the GitHub calls need: a GET with optional headers, at most 3 redirects, a body cap. The token is sent
/// as a Bearer header and never put in an error.
pub trait Fetch {
    fn get(
        &self,
        url: &str,
        token: Option<&str>,
        accept: Option<&str>,
        limit: u64,
    ) -> Result<Vec<u8>, String>;
}

/// The real network, through ureq (rustls, webpki roots).
struct Net;

impl Fetch for Net {
    fn get(
        &self,
        url: &str,
        token: Option<&str>,
        accept: Option<&str>,
        limit: u64,
    ) -> Result<Vec<u8>, String> {
        let mut builder = ureq::http::Request::builder()
            .method("GET")
            .uri(url)
            .header("user-agent", "agentplaza");
        if let Some(accept) = accept {
            builder = builder.header("accept", accept);
        }
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let request = builder
            .body(Vec::new())
            .map_err(|_| "bad request".to_string())?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(FETCH_TIMEOUT))
            .max_redirects(3)
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let mut response = agent
            .run(request)
            .map_err(|e| format!("did not answer: {e}"))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("answered {status}"));
        }
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| format!("reading the reply: {e}"))
    }
}

fn infos(files: &[Entry]) -> Vec<FileInfo> {
    files
        .iter()
        .map(|f| FileInfo {
            path: f.path.clone(),
            size: f.bytes.len() as u64,
            executable: f.executable,
            text: if f.bytes.len() <= 1024 * 1024 {
                String::from_utf8(f.bytes.clone()).ok()
            } else {
                None
            },
        })
        .collect()
}

fn files_in(files: &[Entry]) -> Vec<FileIn> {
    files
        .iter()
        .map(|f| FileIn {
            path: f.path.clone(),
            body: FileBody::Bytes(f.bytes.clone()),
            executable: f.executable,
        })
        .collect()
}

/// The frontmatter name, else the folder's name, cut to the title limit.
fn title_of(dir: &str, name: Option<&str>) -> String {
    let raw = name
        .map(str::to_string)
        .unwrap_or_else(|| dir.rsplit('/').next().unwrap_or("").to_string());
    let title: String = raw.chars().take(MAX_TITLE).collect();
    if title.trim().is_empty() {
        "skill".to_string()
    } else {
        title
    }
}

/// A picked skill's source: the GitHub source with this folder's path, or an upload.
fn source_for(source: &Source, dir: &str) -> Source {
    match source {
        Source::Github {
            repo,
            git_ref,
            commit,
            ..
        } => Source::Github {
            repo: repo.clone(),
            git_ref: git_ref.clone(),
            path: dir.to_string(),
            commit: commit.clone(),
        },
        Source::Built | Source::Upload => Source::Upload,
    }
}

fn origin_text(source: &Source) -> String {
    match source {
        Source::Github {
            repo, commit, path, ..
        } => format!("{repo}@{} {path}", short(commit)),
        _ => "a zip upload".to_string(),
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::Blobs;
    use crate::skills::store_db::DbSkills;
    use std::sync::Arc;

    /// File bytes in memory, by their BLAKE3 hash.
    #[derive(Default)]
    struct TestBlobs {
        map: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl Blobs for TestBlobs {
        fn put(&self, bytes: &[u8]) -> Result<String, String> {
            let hash = apz_blobs::Hash::of(bytes).to_string();
            self.map
                .lock()
                .unwrap()
                .insert(hash.clone(), bytes.to_vec());
            Ok(hash)
        }

        fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
            self.map
                .lock()
                .unwrap()
                .get(hash)
                .cloned()
                .ok_or_else(|| format!("no blob {hash}"))
        }
    }

    /// A GitHub stand-in: the ref answers with `head`; each commit's zipball is in `archives` by URL.
    struct FakeGithub {
        head: Mutex<String>,
        archives: HashMap<String, Vec<u8>>,
    }

    impl Fetch for FakeGithub {
        fn get(
            &self,
            url: &str,
            _token: Option<&str>,
            accept: Option<&str>,
            _limit: u64,
        ) -> Result<Vec<u8>, String> {
            if url.starts_with("https://api.github.com/repos/o/r/commits/") {
                assert_eq!(accept, Some("application/vnd.github.sha"));
                return Ok(self.head.lock().unwrap().clone().into_bytes());
            }
            self.archives
                .get(url)
                .cloned()
                .ok_or_else(|| "answered 404".to_string())
        }
    }

    fn library() -> Skills {
        Skills::with_generations(
            Arc::new(DbSkills::for_tests()),
            Arc::new(TestBlobs::default()),
            Arc::default(),
        )
    }

    fn zip_of(dir: &str, skill_md: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let found = FoundSkill {
            dir: dir.to_string(),
            skill_md: skill_md.to_string(),
            files: files
                .iter()
                .map(|(path, bytes)| Entry {
                    path: path.to_string(),
                    bytes: bytes.to_vec(),
                    executable: false,
                })
                .collect(),
        };
        write_zip(&[found]).unwrap()
    }

    fn archive_url(sha: &str) -> String {
        format!("https://codeload.github.com/o/r/zip/{sha}")
    }

    #[test]
    fn zip_preview_commit_and_export_round_trip() {
        let lib = library();
        let bus = Bus::default();
        let previews = Previews::new();
        let now = 1_000;
        let md = apz_skill::template().to_string();
        let zip = zip_of("folder", &md, &[("notes/readme.txt", b"hello")]);

        let preview = zip_preview(&lib, &previews, &zip, now).unwrap();
        assert_eq!(preview["skills"][0]["path"], "folder");
        let preview_id = preview["preview_id"].as_str().unwrap().to_string();

        let body = CommitBody {
            preview_id: preview_id.clone(),
            picks: vec!["folder".into()],
            bindings: vec!["plaza".into()],
        };
        let made = commit(&lib, &bus, &previews, body, now).unwrap();
        let id = made["skills"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(made["skills"][0]["bindings"], json!(["plaza"]));

        let (single, file_name) = export_version(&lib, &id, "1").unwrap();
        assert!(file_name.ends_with(".zip"));
        let entries = read_zip(&single, Limits::ARCHIVE).unwrap();
        assert!(entries.iter().any(|e| e.path.ends_with("/SKILL.md")));
        assert!(entries.iter().any(|e| e.path.ends_with("notes/readme.txt")));

        let (many, name) = export_many(&lib, &id).unwrap();
        assert_eq!(name, "skills.zip");
        assert_eq!(
            read_zip(&many, Limits::ARCHIVE).unwrap().len(),
            entries.len()
        );

        // The name is live now: the same pick again is refused, and nothing is created.
        let again = CommitBody {
            preview_id,
            picks: vec!["folder".into()],
            bindings: vec![],
        };
        assert!(matches!(
            commit(&lib, &bus, &previews, again, now),
            Err(Fail::Store(StoreError::Conflict(_)))
        ));
        // An unknown preview is a 404.
        let unknown = CommitBody {
            preview_id: "nope".into(),
            picks: vec!["folder".into()],
            bindings: vec![],
        };
        assert!(matches!(
            commit(&lib, &bus, &previews, unknown, now),
            Err(Fail::Status(404, _))
        ));
    }

    #[test]
    fn github_preview_check_and_update_follow_the_ref() {
        let lib = library();
        let bus = Bus::default();
        let previews = Previews::new();
        let now = 1_000;
        let md_a = apz_skill::template().to_string();
        let md_b = format!("{md_a}\nA newer line.\n");
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let gh_zip = |sha: &str, md: &str| {
            zip_of(
                &format!("r-{sha}/skills/foo"),
                md,
                &[("scripts/run.sh", b"echo hi")],
            )
        };
        let fake = FakeGithub {
            head: Mutex::new(sha_a.clone()),
            archives: HashMap::from([
                (archive_url(&sha_a), gh_zip(&sha_a, &md_a)),
                (archive_url(&sha_b), gh_zip(&sha_b, &md_b)),
            ]),
        };

        let preview = github_preview(
            &lib,
            &previews,
            &fake,
            None,
            "https://github.com/o/r/tree/main/skills",
            now,
        )
        .unwrap();
        assert_eq!(preview["skills"][0]["path"], "skills/foo");
        let body = CommitBody {
            preview_id: preview["preview_id"].as_str().unwrap().to_string(),
            picks: vec!["skills/foo".into()],
            bindings: vec![],
        };
        let made = commit(&lib, &bus, &previews, body, now).unwrap();
        let id = made["skills"][0]["id"].as_str().unwrap().to_string();

        let check_now = github_check(&lib, &fake, None, &id).unwrap();
        assert_eq!(check_now["update_available"], false);

        *fake.head.lock().unwrap() = sha_b.clone();
        let check_later = github_check(&lib, &fake, None, &id).unwrap();
        assert_eq!(check_later["update_available"], true);
        assert_eq!(check_later["latest"], sha_b.as_str());

        let updated = github_update(&lib, &bus, &fake, None, &id, now).unwrap();
        assert_eq!(updated["created"], true);
        assert_eq!(lib.store().versions(&id).unwrap().len(), 2);

        let unchanged = github_update(&lib, &bus, &fake, None, &id, now).unwrap();
        assert_eq!(unchanged["created"], false);
    }

    #[test]
    fn an_archive_over_100_mb_is_refused_with_413() {
        let big = vec![0u8; MAX_UPLOAD + 1];
        assert!(matches!(
            zip_preview(&library(), &Previews::new(), &big, 0),
            Err(Fail::Status(413, _))
        ));
    }

    #[test]
    fn previews_expire_and_are_capped() {
        let p = Previews::new();
        let mk = |expires_ms| Preview {
            source: Source::Upload,
            skills: Vec::new(),
            expires_ms,
        };
        let id = p.put(mk(100), 0);
        assert!(p.get(&id, 99).is_some());
        assert!(p.get(&id, 100).is_none());
        for i in 0..20 {
            p.put(mk(1000 + i), 50);
        }
        assert!(p.inner.lock().unwrap().len() <= MAX_PREVIEWS);
    }
}
