//! The plaza's git host: Forgejo (in Docker, 127.0.0.1:3300), reached three ways.
//!
//! - Agents run git in their VMs against `http://plaza.gateway/git/<owner>/<repo>.git`. The hub
//!   relays plaza.gateway to apid as usual; apid forwards git's smart-HTTP to Forgejo signed as
//!   that agent's own Forgejo user. The agent never holds a credential.
//! - Agents use `plaza_git` for the forge itself: repos, pull requests, reviews, merges — apid
//!   calls Forgejo's API as the admin with `Sudo: <agent user>`, so every act is the agent's.
//! - The console's git pages call `/v1/git/...`, which apid answers from Forgejo as the admin.
//!
//! Every agent has a Forgejo user `agent-<cid>` (full name = its plaza name) in the org
//! `agentplaza`, whose team "agents" can write to every repo. Projects get a repo when opened.
//! Configuration comes from keys.env: FORGEJO_URL, FORGEJO_TOKEN (the admin's API token).

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::Agent;
use crate::internal::authorized;
use crate::state::App;

/// The org every plaza repo lives in.
pub const ORG: &str = "agentplaza";
/// The host tool this module serves.
pub const TOOLS: &[&str] = &["plaza_git"];

/// One agent's Forgejo identity. The token is the agent's own access token, used only by apid
/// for the git relay; it never leaves the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub cid: u64,
    pub user: String,
    pub token: String,
}

/// The forge client and the agents' identities (`<state>/git-users.json`, mode 0600).
pub struct Forge {
    /// None when FORGEJO_URL/FORGEJO_TOKEN are not set: every call says the git host is not configured.
    base: Option<String>,
    admin_token: Option<String>,
    path: PathBuf,
    users: Mutex<BTreeMap<u64, Identity>>,
    /// The id of the "agents" team, once looked up or created.
    team: Mutex<Option<u64>>,
}

/// How long one call to Forgejo may take; a large clone or push can run for minutes.
const TIMEOUT: Duration = Duration::from_secs(600);

impl Forge {
    pub fn load(state_dir: &Path, env: &BTreeMap<String, String>) -> Self {
        let path = state_dir.join("git-users.json");
        // A missing file means no identities yet.
        let known: Vec<Identity> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            base: env.get("FORGEJO_URL").cloned(),
            admin_token: env.get("FORGEJO_TOKEN").cloned(),
            path,
            users: Mutex::new(known.into_iter().map(|identity| (identity.cid, identity)).collect()),
            team: Mutex::new(None),
        }
    }

    pub fn configured(&self) -> bool {
        self.base.is_some() && self.admin_token.is_some()
    }

    /// The base URL and the admin's token, or the not-configured error.
    fn creds(&self) -> Result<(&str, &str), String> {
        match (self.base.as_deref(), self.admin_token.as_deref()) {
            (Some(base), Some(token)) => Ok((base, token)),
            _ => Err("the git host is not configured (FORGEJO_URL, FORGEJO_TOKEN)".into()),
        }
    }

    /// One Forgejo API call as the admin, or as `sudo` (a Forgejo user name). `path` starts with
    /// `/api/v1/`. Returns the status and the JSON body (Null when empty).
    pub fn api(&self, method: &str, path: &str, body: Option<&Value>, sudo: Option<&str>) -> Result<(u16, Value), String> {
        let (base, token) = self.creds()?;
        let mut headers = vec![
            ("Authorization", format!("token {token}")),
            ("Content-Type", "application/json".to_string()),
            ("Accept", "application/json".to_string()),
        ];
        if let Some(user) = sudo {
            headers.push(("Sudo", user.to_string()));
        }
        let payload = match body {
            Some(value) => serde_json::to_vec(value).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let (status, _, bytes) = send(method, &url, &headers, payload)?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        Ok((status, value))
    }

    /// The org and the "agents" team (write to all repos), created when missing.
    pub fn ensure_org(&self) -> Result<(), String> {
        let (status, body) = self.api("GET", &format!("/api/v1/orgs/{ORG}"), None, None)?;
        if status == 404 {
            let org = json!({"username": ORG, "visibility": "private"});
            let (status, body) = self.api("POST", "/api/v1/orgs", Some(&org), None)?;
            if !(200..300).contains(&status) {
                return Err(format!("creating the org {ORG} failed ({status}): {body}"));
            }
        } else if !(200..300).contains(&status) {
            return Err(format!("reading the org {ORG} failed ({status}): {body}"));
        }
        self.team_id().map(|_| ())
    }

    /// The id of the "agents" team in the org, found or created; cached after the first look.
    fn team_id(&self) -> Result<u64, String> {
        let cached = *self.team.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = cached {
            return Ok(id);
        }
        let (status, teams) = self.api("GET", &format!("/api/v1/orgs/{ORG}/teams"), None, None)?;
        if !(200..300).contains(&status) {
            return Err(format!("listing the teams of {ORG} failed ({status}): {teams}"));
        }
        let existing = teams
            .as_array()
            .and_then(|list| list.iter().find(|team| team["name"].as_str() == Some("agents")))
            .and_then(|team| team["id"].as_u64());
        let id = match existing {
            Some(id) => id,
            None => {
                let team = json!({
                    "name": "agents",
                    "permission": "write",
                    "includes_all_repositories": true,
                    "units": ["repo.code", "repo.issues", "repo.pulls", "repo.releases", "repo.wiki"]
                });
                let (status, created) = self.api("POST", &format!("/api/v1/orgs/{ORG}/teams"), Some(&team), None)?;
                if !(200..300).contains(&status) {
                    return Err(format!("creating the agents team failed ({status}): {created}"));
                }
                created["id"].as_u64().ok_or_else(|| "the new agents team has no id".to_string())?
            }
        };
        *self.team.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
        Ok(id)
    }

    /// The agent's identity, creating its Forgejo user (and token) and team membership on first use.
    /// `name` is the agent's plaza name (or role) for the user's full name.
    pub fn identity(&self, cid: u64, name: &str) -> Result<Identity, String> {
        // Held across the creation so two calls for one agent cannot make two users or tokens.
        let mut users = self.users.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = users.get(&cid) {
            return Ok(known.clone());
        }
        self.ensure_org()?;
        let team = self.team_id()?;
        let user = format!("agent-{cid}");
        // The password lives only for this call: Forgejo makes a user's token only with that
        // user's own basic auth, so it is set (or reset, for a user made earlier) right here.
        let password = random_password()?;
        let account = json!({
            "username": user,
            "email": format!("{user}@agentplaza.local"),
            "password": password,
            "must_change_password": false,
            "full_name": name
        });
        let (status, body) = self.api("POST", "/api/v1/admin/users", Some(&account), None)?;
        let exists = status == 409 || (status == 422 && body.to_string().contains("already exists"));
        if !(200..300).contains(&status) && !exists {
            return Err(format!("creating {user} failed ({status}): {body}"));
        }
        if exists {
            let reset = json!({ "login_name": user, "source_id": 0, "password": password, "must_change_password": false });
            let (status, body) = self.api("PATCH", &format!("/api/v1/admin/users/{user}"), Some(&reset), None)?;
            if !(200..300).contains(&status) {
                return Err(format!("resetting {user} failed ({status}): {body}"));
            }
        }
        let token = json!({
            "name": format!("apid-relay-{}", now_ms()),
            "scopes": ["write:repository", "write:user", "read:organization"]
        });
        let base = self.base.as_deref().ok_or("the git host is not configured")?;
        let headers = [
            ("Authorization", basic(&user, &password)),
            ("Content-Type", "application/json".to_string()),
            ("Accept", "application/json".to_string()),
        ];
        let (status, _, bytes) = send("POST", &format!("{base}/api/v1/users/{user}/tokens"), &headers, token.to_string().into_bytes())?;
        let reply: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            return Err(format!("creating {user}'s token failed ({status}): {reply}"));
        }
        let token = reply["sha1"].as_str().ok_or_else(|| "the token reply has no sha1".to_string())?.to_string();
        let (status, body) = self.api("PUT", &format!("/api/v1/teams/{team}/members/{user}"), None, None)?;
        if !(200..300).contains(&status) {
            return Err(format!("adding {user} to the agents team failed ({status}): {body}"));
        }
        let identity = Identity { cid, user, token };
        users.insert(cid, identity.clone());
        self.save(&users)?;
        Ok(identity)
    }

    /// Writes the identities as a JSON array, mode 0600 (they hold the agents' tokens).
    fn save(&self, users: &BTreeMap<u64, Identity>) -> Result<(), String> {
        let list: Vec<&Identity> = users.values().collect();
        let bytes = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
        let path = self.path.display();
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&self.path)
            .map_err(|e| format!("opening {path}: {e}"))?;
        file.write_all(&bytes).map_err(|e| format!("writing {path}: {e}"))?;
        std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("protecting {path}: {e}"))
    }

    /// Creates `ORG/<name>` (private, default branch main, auto-init with a README holding
    /// `description`) unless it exists; returns its full name.
    pub fn create_repo(&self, name: &str, description: &str) -> Result<String, String> {
        self.ensure_org()?;
        let repo = json!({
            "name": name,
            "description": description,
            "private": true,
            "auto_init": true,
            "default_branch": "main",
            "readme": "Default"
        });
        let (status, body) = self.api("POST", &format!("/api/v1/orgs/{ORG}/repos"), Some(&repo), None)?;
        if !(200..300).contains(&status) && status != 409 {
            return Err(format!("creating {ORG}/{name} failed ({status}): {body}"));
        }
        Ok(format!("{ORG}/{name}"))
    }
}

/// The relay for git's smart-HTTP from an agent's VM: `GET .../info/refs?service=...`,
/// `POST .../git-upload-pack`, `POST .../git-receive-pack`, forwarded to
/// `<FORGEJO_URL>/<owner>/<repo>/...` with the agent's basic auth; the reply's status, content
/// type and body come back as they are. A request with no Content-Length (chunked) is refused
/// with a hint to set `git config --global http.postBuffer 524288000`.
fn relay(app: &App, req: &Request, cid: u64, owner: &str, repo: &str, tail: &str) -> Response {
    if let Err(e) = app.forge.creds() {
        return Response::error(503, &e);
    }
    // A push without a length cannot be replayed to Forgejo, so git must send one (a small pack does).
    if req.method == "POST" && (req.body.is_empty() || req.header("content-length").is_none()) {
        return Response::error(411, "set `git config --global http.postBuffer 524288000` and retry");
    }
    let name = match app.agents.get(cid) {
        Some(agent) if !agent.name.is_empty() => agent.name,
        Some(agent) => agent.role,
        None => format!("agent-{cid}"),
    };
    let identity = match app.forge.identity(cid, &name) {
        Ok(identity) => identity,
        Err(e) => return Response::error(502, &e),
    };
    let (base, _) = match app.forge.creds() {
        Ok(creds) => creds,
        Err(e) => return Response::error(503, &e),
    };
    let mut url = format!("{}/{owner}/{repo}/{tail}", base.trim_end_matches('/'));
    if !req.query.is_empty() {
        url.push('?');
        url.push_str(&req.query);
    }
    let mut headers = vec![("Authorization", basic(&identity.user, &identity.token))];
    for key in ["Content-Type", "Accept", "Git-Protocol"] {
        if let Some(value) = req.header(key) {
            headers.push((key, value.to_string()));
        }
    }
    match send(&req.method, &url, &headers, req.body.clone()) {
        Ok((status, content_type, bytes)) => Response::new(status, &content_type, bytes),
        Err(e) => Response::error(502, &e),
    }
}

/// One HTTP request: the status, the content type and the body bytes of the reply.
fn send(method: &str, url: &str, headers: &[(&str, String)], body: Vec<u8>) -> Result<(u16, String, Vec<u8>), String> {
    let mut builder = ureq::http::Request::builder().method(method).uri(url);
    for (key, value) in headers {
        builder = builder.header(*key, value.as_str());
    }
    let request = builder.body(body).map_err(|e| format!("bad request: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let response = agent.run(request).map_err(|e| format!("the git host did not answer: {e}"))?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading the git host's reply: {e}"))?;
    Ok((status, content_type, bytes))
}

/// `Basic` authorization for a Forgejo user and its token.
fn basic(user: &str, token: &str) -> String {
    format!("Basic {}", base64(format!("{user}:{token}").as_bytes()))
}

/// Standard base64 with padding (a copy of connectors.rs's, to keep this module self-contained).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A 32-character random password from /dev/urandom (the agent never logs in with it).
fn random_password() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| format!("reading /dev/urandom: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        // The agents' git relay (through the hub's plaza.gateway route).
        .get("/internal/v1/agents/{cid}/gateway/git/{owner}/{repo}/info/refs", |app, req, p| {
            internal(app, req, p, "info/refs")
        })
        .post("/internal/v1/agents/{cid}/gateway/git/{owner}/{repo}/git-upload-pack", |app, req, p| {
            internal(app, req, p, "git-upload-pack")
        })
        .post("/internal/v1/agents/{cid}/gateway/git/{owner}/{repo}/git-receive-pack", |app, req, p| {
            internal(app, req, p, "git-receive-pack")
        })
        // The console's git pages.
        .get("/v1/git", |app, _, _| Response::json(200, &json!({ "configured": app.forge.configured(), "org": ORG })))
        .get("/v1/git/repos", repos_route)
        .post("/v1/git/repos", create_repo_route)
        .get("/v1/git/repos/{repo}", repo_route)
        .get("/v1/git/repos/{repo}/branches", branches_route)
        .get("/v1/git/repos/{repo}/commits", commits_route)
        .get("/v1/git/repos/{repo}/tree", tree_route)
        .get("/v1/git/repos/{repo}/file", file_route)
        .get("/v1/git/repos/{repo}/pulls", pulls_route)
        .get("/v1/git/repos/{repo}/pulls/{n}", pull_route)
        .post("/v1/git/repos/{repo}/pulls/{n}/review", review_route)
        .post("/v1/git/repos/{repo}/pulls/{n}/merge", merge_route)
}

/// Checks the internal token and the cid as the other gateway routes do, then relays.
fn internal(app: &App, req: &Request, p: &Params<'_>, tail: &str) -> Response {
    authorized(app, req, p.get("cid"), |cid| Ok(relay(app, req, cid, p.get("owner"), p.get("repo"), tail)))
}

// The console's routes. Each answers in a small stable shape (not Forgejo's raw JSON):
// repos:    [{name, description, default_branch, updated_at, size_kb, open_prs, project: Option<id>}]
// repo:     {name, description, default_branch, updated_at, clone_url_for_agents}
// branches: [{name, commit: {id, message, author, at}}]
// commits:  [{id, short, message, author, at, additions, deletions}]   (?ref=&limit=, default 30)
// tree:     [{name, path, kind: "file"|"dir", size}]                  (?ref=&path=)
// file:     {path, ref, size, text: Option<String>, binary: bool}     (?ref=&path=; text ≤ 512 KiB)
// pulls:    [{number, title, state, author, head, base, created_at, updated_at, comments, review: "approved"|"changes_requested"|"pending", mergeable}] (?state=open|closed|all)
// pull:     the row above + {body, diff (≤ 1 MiB unified diff), reviews: [{author, state, body, at}], comments: [{author, body, at, path?, line?}], files: [{path, additions, deletions}]}
// review:   body {event: "APPROVE"|"REQUEST_CHANGES"|"COMMENT", body} — as the owner (admin)
// merge:    body {style?: "merge"|"squash"|"rebase"}
// create:   body {name, description}
/// A failed forge call: the status to answer with and the message.
struct Failure {
    status: u16,
    message: String,
}

impl Failure {
    fn new(status: u16, message: &str) -> Self {
        Self { status, message: message.to_string() }
    }
}

const RUN_DIFF: usize = 20 * 1024;
const ROUTE_DIFF: usize = 1024 * 1024;
const TEXT_LIMIT: usize = 512 * 1024;

fn reply(result: Result<Value, Failure>, ok: u16) -> Response {
    match result {
        Ok(v) => Response::json(ok, &v),
        Err(f) => Response::error(f.status, &f.message),
    }
}

/// A query value, percent-decoded; None when absent or empty.
fn q(req: &Request, key: &str) -> Option<String> {
    req.query_param(key).map(unescape).filter(|v| !v.is_empty())
}

/// One Forgejo call, as `sudo` when given, else as the admin. Non-2xx answers carry Forgejo's message.
fn forge(app: &App, method: &str, path: &str, body: Option<&Value>, sudo: Option<&str>) -> Result<Value, Failure> {
    if !app.forge.configured() {
        return Err(Failure::new(503, "the git host is not configured"));
    }
    let (status, value) = app.forge.api(method, path, body, sudo).map_err(|e| Failure::new(503, &e))?;
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let message = at(&value, "message");
    Err(Failure::new(status, if message.is_empty() { "git host error" } else { message }))
}

fn at<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn num(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn need<'a>(input: &'a Value, key: &str) -> Result<&'a str, Failure> {
    match input[key].as_str() {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(Failure::new(400, &format!("{key} is required"))),
    }
}

fn number(raw: &str) -> Result<u64, Failure> {
    raw.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| Failure::new(400, "n must be a pull request number"))
}

fn number_arg(input: &Value) -> Result<u64, Failure> {
    input["number"].as_u64().filter(|n| *n > 0).ok_or_else(|| Failure::new(400, "number is required"))
}

/// Percent-encodes for a URL; `/` stays when `slash` is set.
fn pct(raw: &str, slash: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || (slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

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

fn unescape(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1).and_then(|c| hex(*c)), b.get(i + 2).and_then(|c| hex(*c))) {
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

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return Err("the file is not valid base64".into()),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// `s` cut to at most `max` bytes on a character boundary.
fn clip(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").to_string()
}

fn lines(items: Vec<String>, empty: &str) -> String {
    if items.is_empty() {
        empty.to_string()
    } else {
        items.join("\n")
    }
}

fn contents_path(repo: &str, path: &str, reference: Option<&str>) -> String {
    let mut out = format!("/api/v1/repos/{ORG}/{}/contents", seg(repo));
    let path = path.trim_matches('/');
    if !path.is_empty() {
        out.push('/');
        out.push_str(&pct(path, true));
    }
    if let Some(r) = reference {
        out.push_str("?ref=");
        out.push_str(&seg(r));
    }
    out
}

/// The review state of a pull request: the last non-COMMENT state per reviewer.
fn review_state(reviews: &Value) -> &'static str {
    let mut last: BTreeMap<String, String> = BTreeMap::new();
    for r in arr(reviews) {
        let state = at(r, "state");
        if matches!(state, "COMMENT" | "PENDING") || r["dismissed"].as_bool() == Some(true) {
            continue;
        }
        last.insert(at(&r["user"], "login").to_string(), state.to_string());
    }
    if last.values().any(|s| s == "REQUEST_CHANGES") {
        "changes_requested"
    } else if last.values().any(|s| s == "APPROVED") {
        "approved"
    } else {
        "pending"
    }
}

fn repo_shape(r: &Value) -> Value {
    let name = at(r, "name");
    json!({
        "name": name,
        "description": at(r, "description"),
        "default_branch": at(r, "default_branch"),
        "updated_at": at(r, "updated_at"),
        "clone_url_for_agents": format!("http://plaza.gateway/git/{ORG}/{name}.git")
    })
}

fn pr_row(p: &Value, review: &str) -> Value {
    json!({
        "number": num(p, "number"),
        "title": at(p, "title"),
        "state": at(p, "state"),
        "author": at(&p["user"], "login"),
        "head": at(&p["head"], "ref"),
        "base": at(&p["base"], "ref"),
        "created_at": at(p, "created_at"),
        "updated_at": at(p, "updated_at"),
        "comments": num(p, "comments"),
        "review": review,
        "mergeable": p["mergeable"].as_bool().unwrap_or(false)
    })
}

fn list_repos(app: &App, sudo: Option<&str>) -> Result<Value, Failure> {
    let raw = forge(app, "GET", &format!("/api/v1/orgs/{ORG}/repos?limit=100"), None, sudo)?;
    let projects = app.projects.list();
    let rows = arr(&raw)
        .iter()
        .map(|r| {
            let full = format!("{ORG}/{}", at(r, "name"));
            let project = projects.iter().find(|p| p.repo.as_deref() == Some(full.as_str())).map(|p| p.id.clone());
            json!({
                "name": at(r, "name"),
                "description": at(r, "description"),
                "default_branch": at(r, "default_branch"),
                "updated_at": at(r, "updated_at"),
                "size_kb": num(r, "size"),
                "open_prs": num(r, "open_pr_counter"),
                "project": project
            })
        })
        .collect();
    Ok(Value::Array(rows))
}

fn get_repo(app: &App, sudo: Option<&str>, repo: &str) -> Result<Value, Failure> {
    let r = forge(app, "GET", &format!("/api/v1/repos/{ORG}/{}", seg(repo)), None, sudo)?;
    Ok(repo_shape(&r))
}

fn create(app: &App, name: &str, description: &str) -> Result<Value, Failure> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return Err(Failure::new(400, "name is required (a plain repository name)"));
    }
    let full = app.forge.create_repo(name, description).map_err(|e| Failure::new(502, &e))?;
    let r = forge(app, "GET", &format!("/api/v1/repos/{}", pct(&full, true)), None, None)?;
    Ok(repo_shape(&r))
}

fn branches(app: &App, sudo: Option<&str>, repo: &str) -> Result<Value, Failure> {
    let raw = forge(app, "GET", &format!("/api/v1/repos/{ORG}/{}/branches", seg(repo)), None, sudo)?;
    let rows = arr(&raw)
        .iter()
        .map(|b| {
            let c = &b["commit"];
            json!({
                "name": at(b, "name"),
                "commit": {
                    "id": at(c, "id"),
                    "message": first_line(at(c, "message")),
                    "author": at(&c["author"], "name"),
                    "at": at(c, "timestamp")
                }
            })
        })
        .collect();
    Ok(Value::Array(rows))
}

fn commits(app: &App, sudo: Option<&str>, repo: &str, reference: Option<String>, limit: usize) -> Result<Value, Failure> {
    let sha = reference.map(|r| format!("&sha={}", seg(&r))).unwrap_or_default();
    let path = format!("/api/v1/repos/{ORG}/{}/commits?limit={limit}&stat=true{sha}", seg(repo));
    let raw = forge(app, "GET", &path, None, sudo)?;
    let rows = arr(&raw)
        .iter()
        .map(|c| {
            let id = at(c, "sha");
            let commit = &c["commit"];
            json!({
                "id": id,
                "short": id.chars().take(8).collect::<String>(),
                "message": first_line(at(commit, "message")),
                "author": at(&commit["author"], "name"),
                "at": at(&commit["author"], "date"),
                "additions": num(&c["stats"], "additions"),
                "deletions": num(&c["stats"], "deletions")
            })
        })
        .collect();
    Ok(Value::Array(rows))
}

fn tree(app: &App, sudo: Option<&str>, repo: &str, path: &str, reference: Option<String>) -> Result<Value, Failure> {
    let raw = forge(app, "GET", &contents_path(repo, path, reference.as_deref()), None, sudo)?;
    let mut rows: Vec<Value> = arr(&raw)
        .iter()
        .map(|e| {
            let kind = if at(e, "type") == "dir" { "dir" } else { "file" };
            json!({"name": at(e, "name"), "path": at(e, "path"), "kind": kind, "size": num(e, "size")})
        })
        .collect();
    rows.sort_by(|a, b| {
        (at(b, "kind") == "dir").cmp(&(at(a, "kind") == "dir")).then_with(|| at(a, "name").cmp(at(b, "name")))
    });
    Ok(Value::Array(rows))
}

fn file(app: &App, sudo: Option<&str>, repo: &str, path: &str, reference: Option<String>) -> Result<Value, Failure> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Err(Failure::new(400, "path is required"));
    }
    let raw = forge(app, "GET", &contents_path(repo, path, reference.as_deref()), None, sudo)?;
    if raw.is_array() {
        return Err(Failure::new(400, "that path is a directory"));
    }
    let bytes = base64_decode(at(&raw, "content")).map_err(|e| Failure::new(502, &e))?;
    let text = if bytes.len() <= TEXT_LIMIT { String::from_utf8(bytes).ok() } else { None };
    let binary = text.is_none();
    Ok(json!({
        "path": path,
        "ref": reference.unwrap_or_default(),
        "size": num(&raw, "size"),
        "text": text,
        "binary": binary
    }))
}

fn pulls(app: &App, sudo: Option<&str>, repo: &str, state: &str) -> Result<Value, Failure> {
    let raw = forge(app, "GET", &format!("/api/v1/repos/{ORG}/{}/pulls?state={}&limit=50", seg(repo), seg(state)), None, sudo)?;
    let rows = arr(&raw);
    let mut out = Vec::with_capacity(rows.len());
    for p in rows {
        // The review state costs one call per pull request, so it is computed for up to 20 only.
        let review = if rows.len() <= 20 {
            let path = format!("/api/v1/repos/{ORG}/{}/pulls/{}/reviews", seg(repo), num(p, "number"));
            review_state(&forge(app, "GET", &path, None, sudo)?)
        } else {
            "pending"
        };
        out.push(pr_row(p, review));
    }
    Ok(Value::Array(out))
}

fn pull(app: &App, sudo: Option<&str>, repo: &str, n: u64) -> Result<Value, Failure> {
    let base = format!("/api/v1/repos/{ORG}/{}/pulls/{n}", seg(repo));
    let p = forge(app, "GET", &base, None, sudo)?;
    let reviews = forge(app, "GET", &format!("{base}/reviews"), None, sudo)?;
    let comments = forge(app, "GET", &format!("/api/v1/repos/{ORG}/{}/issues/{n}/comments", seg(repo)), None, sudo)?;
    let files = forge(app, "GET", &format!("{base}/files"), None, sudo)?;
    let mut diff = String::new();
    let mut file_rows = Vec::new();
    for f in arr(&files) {
        file_rows.push(json!({"path": at(f, "filename"), "additions": num(f, "additions"), "deletions": num(f, "deletions")}));
        if let Some(patch) = f["patch"].as_str() {
            diff.push_str(patch);
            if !patch.ends_with('\n') {
                diff.push('\n');
            }
        }
    }
    let mut row = pr_row(&p, review_state(&reviews));
    row["body"] = json!(at(&p, "body"));
    row["diff"] = json!(clip(diff, ROUTE_DIFF));
    row["reviews"] = Value::Array(
        arr(&reviews)
            .iter()
            .map(|r| json!({"author": at(&r["user"], "login"), "state": at(r, "state"), "body": at(r, "body"), "at": at(r, "submitted_at")}))
            .collect(),
    );
    row["comments"] = Value::Array(
        arr(&comments)
            .iter()
            .map(|c| json!({"author": at(&c["user"], "login"), "body": at(c, "body"), "at": at(c, "created_at")}))
            .collect(),
    );
    row["files"] = Value::Array(file_rows);
    Ok(row)
}

fn review(app: &App, sudo: Option<&str>, repo: &str, n: u64, event: &str, body: &str) -> Result<Value, Failure> {
    let path = format!("/api/v1/repos/{ORG}/{}/pulls/{n}/reviews", seg(repo));
    forge(app, "POST", &path, Some(&json!({"event": event, "body": body})), sudo)?;
    Ok(json!({"number": n, "event": event}))
}

fn comment(app: &App, sudo: Option<&str>, repo: &str, n: u64, body: &str) -> Result<Value, Failure> {
    let path = format!("/api/v1/repos/{ORG}/{}/issues/{n}/comments", seg(repo));
    forge(app, "POST", &path, Some(&json!({"body": body})), sudo)?;
    Ok(json!({"number": n}))
}

fn open_pr(app: &App, sudo: Option<&str>, repo: &str, head: &str, base: &str, title: &str, body: &str) -> Result<Value, Failure> {
    let base = if base.is_empty() { "main" } else { base };
    let path = format!("/api/v1/repos/{ORG}/{}/pulls", seg(repo));
    let v = forge(app, "POST", &path, Some(&json!({"head": head, "base": base, "title": title, "body": body})), sudo)?;
    Ok(json!({"number": num(&v, "number"), "title": at(&v, "title")}))
}

fn merge_pr(app: &App, sudo: Option<&str>, repo: &str, n: u64, style: &str) -> Result<Value, Failure> {
    let path = format!("/api/v1/repos/{ORG}/{}/pulls/{n}/merge", seg(repo));
    forge(app, "POST", &path, Some(&json!({"Do": style})), sudo)?;
    Ok(json!({"number": n, "merged": true}))
}

/// True when the pull request has an APPROVED review by someone other than its author.
fn approved_by_other(app: &App, sudo: Option<&str>, repo: &str, n: u64) -> Result<bool, Failure> {
    let base = format!("/api/v1/repos/{ORG}/{}/pulls/{n}", seg(repo));
    let p = forge(app, "GET", &base, None, sudo)?;
    let reviews = forge(app, "GET", &format!("{base}/reviews"), None, sudo)?;
    let author = at(&p["user"], "login");
    Ok(arr(&reviews).iter().any(|r| {
        at(r, "state") == "APPROVED" && r["dismissed"].as_bool() != Some(true) && at(&r["user"], "login") != author
    }))
}

fn repos_route(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    // The org exists from the first look on, so an empty plaza lists no repos rather than failing.
    if app.forge.configured() {
        if let Err(e) = app.forge.ensure_org() {
            return Response::error(502, &e);
        }
    }
    reply(list_repos(app, None), 200)
}

fn create_repo_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let Ok(body) = req.json::<Value>() else {
        return Response::error(400, "the body must be JSON: {name, description}");
    };
    reply(create(app, at(&body, "name"), at(&body, "description")), 201)
}

fn repo_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    reply(get_repo(app, None, p.get("repo")), 200)
}

fn branches_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    reply(branches(app, None, p.get("repo")), 200)
}

fn commits_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let limit = q(req, "limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(30).clamp(1, 100);
    reply(commits(app, None, p.get("repo"), q(req, "ref"), limit), 200)
}

fn tree_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    reply(tree(app, None, p.get("repo"), &q(req, "path").unwrap_or_default(), q(req, "ref")), 200)
}

fn file_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    reply(file(app, None, p.get("repo"), &q(req, "path").unwrap_or_default(), q(req, "ref")), 200)
}

fn pulls_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let state = q(req, "state").unwrap_or_else(|| "open".to_string());
    reply(pulls(app, None, p.get("repo"), &state), 200)
}

fn pull_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    reply(number(p.get("n")).and_then(|n| pull(app, None, p.get("repo"), n)), 200)
}

fn review_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Ok(body) = req.json::<Value>() else {
        return Response::error(400, "the body must be JSON: {event, body}");
    };
    let event = at(&body, "event");
    if !matches!(event, "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
        return Response::error(400, "event must be APPROVE, REQUEST_CHANGES or COMMENT");
    }
    reply(number(p.get("n")).and_then(|n| review(app, None, p.get("repo"), n, event, at(&body, "body"))), 200)
}

fn merge_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let style = req.json::<Value>().ok().map(|b| at(&b, "style").to_string()).unwrap_or_default();
    let style = if style.is_empty() { "merge".to_string() } else { style };
    if !matches!(style.as_str(), "merge" | "squash" | "rebase") {
        return Response::error(400, "style must be merge, squash or rebase");
    }
    reply(number(p.get("n")).and_then(|n| merge_pr(app, None, p.get("repo"), n, &style)), 200)
}

/// The text of a pull request for the agent: header, files, reviews, and the diff clipped to 20 KB.
fn pull_text(v: &Value) -> String {
    let mut out = format!(
        "#{} {}\n{} by {}: {} -> {}, review: {}, mergeable: {}\n",
        num(v, "number"),
        at(v, "title"),
        at(v, "state"),
        at(v, "author"),
        at(v, "head"),
        at(v, "base"),
        at(v, "review"),
        v["mergeable"].as_bool().unwrap_or(false)
    );
    if !at(v, "body").trim().is_empty() {
        out.push_str(&format!("\n{}\n", at(v, "body")));
    }
    out.push_str("\nfiles:\n");
    for f in arr(&v["files"]) {
        out.push_str(&format!("  {} +{} -{}\n", at(f, "path"), num(f, "additions"), num(f, "deletions")));
    }
    out.push_str("\nreviews:\n");
    for r in arr(&v["reviews"]) {
        out.push_str(&format!("  {}: {}", at(r, "author"), at(r, "state")));
        if !at(r, "body").trim().is_empty() {
            out.push_str(&format!(" - {}", at(r, "body")));
        }
        out.push('\n');
    }
    let diff = at(v, "diff");
    if !diff.is_empty() {
        let shown = clip(diff.to_string(), RUN_DIFF);
        out.push_str(&format!("\ndiff:\n{shown}\n"));
        if shown.len() < diff.len() {
            out.push_str("[diff clipped at 20 KB]\n");
        }
    }
    out
}

/// The plaza_git tool's actions, as `sudo` (the agent's Forgejo user).
fn act(app: &App, sudo: Option<&str>, input: &Value) -> Result<String, Failure> {
    match at(input, "action") {
        "repos" => {
            let repos = list_repos(app, sudo)?;
            Ok(lines(
                arr(&repos).iter().map(|r| format!("{}: {}", at(r, "name"), at(r, "description"))).collect(),
                "no repositories yet",
            ))
        }
        "create_repo" => {
            let r = create(app, need(input, "name")?, at(input, "description"))?;
            Ok(format!("created {}\nclone: {}", at(&r, "name"), at(&r, "clone_url_for_agents")))
        }
        "prs" => {
            let state = input["state"].as_str().unwrap_or("open");
            let rows = pulls(app, sudo, need(input, "repo")?, state)?;
            Ok(lines(
                arr(&rows)
                    .iter()
                    .map(|p| {
                        format!(
                            "#{} [{}] {} ({} -> {}) by {}, review: {}",
                            num(p, "number"),
                            at(p, "state"),
                            at(p, "title"),
                            at(p, "head"),
                            at(p, "base"),
                            at(p, "author"),
                            at(p, "review")
                        )
                    })
                    .collect(),
                "no pull requests",
            ))
        }
        "pr" => Ok(pull_text(&pull(app, sudo, need(input, "repo")?, number_arg(input)?)?)),
        "open_pr" => {
            let v = open_pr(
                app,
                sudo,
                need(input, "repo")?,
                need(input, "head")?,
                input["base"].as_str().unwrap_or(""),
                need(input, "title")?,
                at(input, "body"),
            )?;
            Ok(format!("opened #{} {}", num(&v, "number"), at(&v, "title")))
        }
        "review" => {
            let event = need(input, "event")?;
            if !matches!(event, "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
                return Err(Failure::new(400, "event must be APPROVE, REQUEST_CHANGES or COMMENT"));
            }
            let n = number_arg(input)?;
            review(app, sudo, need(input, "repo")?, n, event, at(input, "body"))?;
            Ok(format!("review {event} posted on #{n}"))
        }
        "comment" => {
            let n = number_arg(input)?;
            comment(app, sudo, need(input, "repo")?, n, need(input, "body")?)?;
            Ok(format!("commented on #{n}"))
        }
        "merge" => {
            let repo = need(input, "repo")?;
            let n = number_arg(input)?;
            if !approved_by_other(app, sudo, repo, n)? {
                return Err(Failure::new(409, "a pull request needs an approving review by another agent before it is merged"));
            }
            merge_pr(app, sudo, repo, n, "merge")?;
            Ok(format!("merged #{n}"))
        }
        other => Err(Failure::new(400, &format!("unknown action '{other}'"))),
    }
}

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    Some(match name {
        "plaza_git" => (
            "The plaza's git forge. In your VM, clone and push with plain git: `git clone http://plaza.gateway/git/agentplaza/<repo>.git` (no credentials needed; you push as yourself). Here: repos (list), create_repo (name, description), prs (repo, state), pr (repo, number: details, diff, reviews), open_pr (repo, head, base, title, body), review (repo, number, event APPROVE|REQUEST_CHANGES|COMMENT, body), comment (repo, number, body), merge (repo, number).",
            obj(json!({
                "action": {"type": "string", "enum": ["repos", "create_repo", "prs", "pr", "open_pr", "review", "comment", "merge"]},
                "repo": {"type": "string"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "state": {"type": "string", "enum": ["open", "closed", "all"]},
                "number": {"type": "integer"},
                "head": {"type": "string"},
                "base": {"type": "string"},
                "title": {"type": "string"},
                "body": {"type": "string"},
                "event": {"type": "string", "enum": ["APPROVE", "REQUEST_CHANGES", "COMMENT"]}
            }), &["action"]),
        ),
        _ => return None,
    })
}

/// Runs `plaza_git` for `agent`, as its Forgejo user (Sudo).
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    if !app.forge.configured() {
        return Err("the git host is not configured".into());
    }
    let user = app.forge.identity(agent.cid, &agent.name)?.user;
    act(app, Some(user.as_str()), input).map_err(|f| f.message)
}
