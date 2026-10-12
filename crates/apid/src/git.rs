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
        let account = json!({
            "username": user,
            "email": format!("{user}@agentplaza.local"),
            "password": random_password()?,
            "must_change_password": false,
            "full_name": name
        });
        let (status, body) = self.api("POST", "/api/v1/admin/users", Some(&account), None)?;
        let exists = status == 409 || (status == 422 && body.to_string().contains("already exists"));
        if !(200..300).contains(&status) && !exists {
            return Err(format!("creating {user} failed ({status}): {body}"));
        }
        let token = json!({
            "name": format!("apid-relay-{}", now_ms()),
            "scopes": ["write:repository", "write:user", "read:organization"]
        });
        let (status, reply) = self.api("POST", &format!("/api/v1/users/{user}/tokens"), Some(&token), Some(&user))?;
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
fn repos_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn create_repo_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn repo_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn branches_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn commits_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn tree_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn file_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn pulls_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn pull_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn review_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }
fn merge_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: G1") }

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
pub fn run(_app: &App, _agent: &Agent, _input: &Value) -> Result<String, String> {
    Err("the git host is not available yet".into()) // contract: G1
}
