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
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::Agent;
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
}

impl Forge {
    pub fn load(state_dir: &Path, env: &BTreeMap<String, String>) -> Self {
        // contract: G1 (read the identities file)
        Self {
            base: env.get("FORGEJO_URL").cloned(),
            admin_token: env.get("FORGEJO_TOKEN").cloned(),
            path: state_dir.join("git-users.json"),
            users: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn configured(&self) -> bool {
        self.base.is_some() && self.admin_token.is_some()
    }

    /// One Forgejo API call as the admin, or as `sudo` (a Forgejo user name). `path` starts with
    /// `/api/v1/`. Returns the status and the JSON body (Null when empty).
    pub fn api(&self, _method: &str, _path: &str, _body: Option<&Value>, _sudo: Option<&str>) -> Result<(u16, Value), String> {
        let _ = (&self.base, &self.admin_token, &self.path);
        Err("the git host is not available yet".into()) // contract: G1
    }

    /// The org and the "agents" team (write to all repos), created when missing.
    pub fn ensure_org(&self) -> Result<(), String> {
        Err("the git host is not available yet".into()) // contract: G1
    }

    /// The agent's identity, creating its Forgejo user (and token) and team membership on first use.
    /// `name` is the agent's plaza name (or role) for the user's full name.
    pub fn identity(&self, _cid: u64, _name: &str) -> Result<Identity, String> {
        let _ = &self.users;
        Err("the git host is not available yet".into()) // contract: G1
    }

    /// Creates `ORG/<name>` (private, default branch main, auto-init with a README holding
    /// `description`) unless it exists; returns its full name.
    pub fn create_repo(&self, _name: &str, _description: &str) -> Result<String, String> {
        Err("the git host is not available yet".into()) // contract: G1
    }
}

/// The relay for git's smart-HTTP from an agent's VM: `GET .../info/refs?service=...`,
/// `POST .../git-upload-pack`, `POST .../git-receive-pack`, forwarded to
/// `<FORGEJO_URL>/<owner>/<repo>/...` with the agent's basic auth; the reply's status, content
/// type and body come back as they are. A request with no Content-Length (chunked) is refused
/// with a hint to set `git config --global http.postBuffer 524288000`.
fn relay(_app: &App, _req: &Request, _cid: u64, _owner: &str, _repo: &str, _tail: &str) -> Response {
    Response::error(501, "the git host is not available yet") // contract: G1
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
fn internal(_app: &App, _req: &Request, _p: &Params<'_>, _tail: &str) -> Response {
    let _ = relay;
    Response::error(501, "the git host is not available yet") // contract: G1
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
