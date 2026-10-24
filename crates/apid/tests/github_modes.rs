//! GitHub's two modes end to end on a scratch hub (PLAN 21.5, 18.4), against stand-ins: a fake GitHub API (a daemon on
//! localhost that records its calls and answers pull requests and check runs) and bare git repositories behind the
//! repos' clone URLs (git over file://). Nothing reaches github.com.
//!
//! - `mirror`: a lane is landed in Weft through its queue, and the landing is exported to the repository's main (one
//!   commit, fast-forward, with the intent's title).
//! - `ci`: the lane is pushed as a branch, opened as a pull request, and gated by the check runs GitHub reports (a
//!   failing check keeps it in the queue; the gate is a run of the `ci` recipe). It lands only once GitHub merges the
//!   pull request, and the merge commit becomes the connection's head. A commit GitHub's main gets on its own is
//!   imported before the next lane is pushed.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

const WAIT: Duration = Duration::from_secs(60);

/// Runs git in `dir` with a fixed author and clock; its trimmed output.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada Owner")
        .env("GIT_AUTHOR_EMAIL", "ada@example.test")
        .env("GIT_COMMITTER_NAME", "Ada Owner")
        .env("GIT_COMMITTER_EMAIL", "ada@example.test")
        .env("GIT_AUTHOR_DATE", "2026-09-01T10:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-09-01T10:00:00Z")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare repository with two commits on main; its path and its file URL (the clone URL GitHub would name).
fn remote(root: &Path, name: &str) -> (PathBuf, String) {
    let work = root.join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    for i in 0..2 {
        std::fs::write(
            work.join(format!("readme-{i}.md")),
            format!("# Readme {i}\n"),
        )
        .unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", &format!("Readme {i}")]);
    }
    let bare = root.join(format!("{name}.git"));
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let url = format!("file://{}", bare.display());
    (bare, url)
}

/// The fake GitHub: what it has (pull requests, check runs) and what it was asked.
#[derive(Default)]
struct GitHub {
    pulls: Vec<Value>,
    checks: Vec<Value>,
    opened: Vec<Value>,
    /// The check runs created (POST bodies) and updated (PATCH bodies, each with its id), oldest first.
    created: Vec<Value>,
    updated: Vec<Value>,
    /// The last check run id given out.
    last_check: u64,
}

type Shared = Arc<Mutex<GitHub>>;

/// The repository a GitHub route names (`/api/v3/repos/JumpHash/<name>/...`).
fn repo_named(route: &str) -> Option<&str> {
    route
        .strip_prefix("/api/v3/repos/JumpHash/")?
        .split('/')
        .next()
}

/// The fake GitHub API at `/api/v3` for the token's account and two repositories (`JumpHash/<name>` → its clone URL).
fn github(repos: BTreeMap<String, String>) -> (FakeDaemon, Shared) {
    let state: Shared = Arc::default();
    let shared = Arc::clone(&state);
    let daemon = FakeDaemon::serve(move |method, path, body| {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let mut st = shared.lock().unwrap_or_else(|p| p.into_inner());
        let reply = match (method, route) {
            ("GET", "/api/v3/user") => (
                200,
                json!({ "id": 5, "login": "irfan", "name": "Irfan S", "email": "irfan@gh.test", "avatar_url": "" }),
            ),
            ("GET", "/api/v3/user/emails") => (
                200,
                json!([{ "email": "irfan@gh.test", "verified": true, "primary": true }]),
            ),
            ("GET", r)
                if repo_named(r).is_some_and(|n| repos.contains_key(n))
                    && r.matches('/').count() == 5 =>
            {
                let name = repo_named(r).unwrap();
                (
                    200,
                    json!({ "full_name": format!("JumpHash/{name}"), "owner": { "login": "JumpHash", "type": "Organization", "id": 9 }, "private": true, "default_branch": "main", "clone_url": repos[name], "html_url": format!("https://github.example/JumpHash/{name}"), "permissions": { "push": true, "pull": true } }),
                )
            }
            ("GET", r) if r.ends_with("/pulls") => {
                let found: Vec<Value> = st
                    .pulls
                    .iter()
                    .filter(|p| {
                        query.contains(&p["head"].as_str().unwrap_or("").replace('/', "%2F"))
                    })
                    .cloned()
                    .collect();
                (200, json!(found))
            }
            ("POST", r) if r.ends_with("/pulls") => {
                let req: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let number = st.pulls.len() as u64 + 1;
                let pull = json!({
                    "number": number, "state": "open", "merged_at": null, "merge_commit_sha": null,
                    "html_url": format!("https://github.example/pull/{number}"), "head": req["head"], "base": req["base"],
                });
                st.opened.push(req);
                st.pulls.push(pull.clone());
                (201, pull)
            }
            ("GET", r) if r.contains("/check-runs") => (
                200,
                json!({ "total_count": st.checks.len(), "check_runs": st.checks }),
            ),
            ("POST", r) if r.ends_with("/check-runs") => {
                let req: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                st.last_check += 1;
                let id = st.last_check;
                st.checks.push(json!({
                    "id": id, "name": req["name"], "status": req["status"], "conclusion": req["conclusion"],
                    "head_sha": req["head_sha"], "output": req["output"], "html_url": format!("https://github.example/check/{id}"),
                }));
                st.created.push(req);
                (201, json!({ "id": id }))
            }
            ("PATCH", r) if r.contains("/check-runs/") => {
                let req: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let id = r.rsplit('/').next().and_then(|n| n.parse::<u64>().ok());
                match st.checks.iter_mut().find(|c| c["id"].as_u64() == id) {
                    Some(check) => {
                        check["status"] = req["status"].clone();
                        check["conclusion"] = req["conclusion"].clone();
                        check["output"] = req["output"].clone();
                        st.updated.push(json!({ "id": id, "body": req }));
                        (200, json!({ "id": id }))
                    }
                    None => (404, json!({ "message": "Not Found" })),
                }
            }
            _ => (404, json!({ "message": "Not Found" })),
        };
        (reply.0, reply.1.to_string())
    });
    (daemon, state)
}

/// A hub with Weft running.
fn hub_with_weft(name: &str) -> (Scratch, Hub) {
    let scratch = Scratch::new(name);
    let hub = Hub::start(&scratch, None, None);
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    (scratch, hub)
}

/// A project opened in `github` mode (the owner's create route), its id.
fn project(hub: &Hub, title: &str, github: &str) -> String {
    let (status, p) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": title, "goal": title, "kind": "other", "github": github })),
    );
    assert_eq!(status, 201, "{p}");
    assert_eq!(p["github"], github, "{p}");
    text(&p["id"])
}

/// A GitHub token for `repo` (`JumpHash/<name>`), as the owner gave it; the authorization's id.
fn token_for(hub: &Hub, gh_host: &str, name: &str) -> String {
    let (status, integ) = hub.call(
        "POST",
        "/v1/platforms/tokens",
        Some(json!({ "kind": "github", "host": gh_host, "token": format!("ghp_{name}"), "level": "repo", "target": format!("JumpHash/{name}") })),
    );
    assert_eq!(status, 200, "{integ}");
    text(&integ["id"])
}

/// Connects `JumpHash/<name>` to `project` (its history imported); Weft's repo name.
fn connect(hub: &Hub, integ: &str, name: &str, project: &str) -> String {
    let (status, out) = hub.call(
        "POST",
        &format!("/v1/platforms/{integ}/connect"),
        Some(json!({ "repo": format!("JumpHash/{name}"), "project": project })),
    );
    assert_eq!(status, 200, "{out}");
    text(&out["weft_repo"])
}

/// The owner's lane on `repo`: one file, one intent, sealed and queued for landing. Its change's name.
fn lane(hub: &Hub, repo: &str, file: &str, body: &str, intent: &str) -> String {
    let bytes = body.as_bytes();
    let hash = weft::hash::Hash::of(bytes);
    let (status, stored) = hub.send_bytes(
        "PUT",
        &format!("/v1/weft/wire/objects/{}", hash.hex()),
        bytes,
        "application/octet-stream",
    );
    assert_eq!(status, 200, "{stored}");
    let (status, opened) = hub.call(
        "POST",
        "/v1/weft/lanes",
        Some(json!({ "repo": repo, "ref": "main", "title": intent, "author": "owner" })),
    );
    assert_eq!(status, 200, "{opened}");
    let change = text(&opened["change"]);
    let deltas = json!([{ "path": file, "after": hash.hex(), "mode": 0o100644 }]);
    for (kind, payload) in [
        (
            "revision.draft",
            json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["test:lane"] }),
        ),
        (
            "revision.seal",
            json!({ "change": change, "n": 1, "deltas": deltas }),
        ),
        (
            "intent.set",
            json!({ "change": change, "n": 1, "intents": [{ "title": intent, "kind": "change", "covers": [file] }] }),
        ),
        (
            "queue.add",
            json!({ "repo": repo, "ref": "main", "change": change, "n": 1 }),
        ),
    ] {
        let (status, out) = hub.call(
            "POST",
            "/v1/weft/console",
            Some(json!({ "kind": kind, "body": payload })),
        );
        assert_eq!(status, 200, "{kind}: {out}\n{}", hub.log());
    }
    change
}

/// The change's state as Weft has it now.
fn state_of(hub: &Hub, change: &str) -> String {
    let (_, detail) = hub.call("GET", &format!("/v1/weft/changes/{change}"), None);
    text(&detail["state"])
}

/// Waits until `ready` holds, else panics with the hub's log.
fn eventually(hub: &Hub, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "{what}\n{}", hub.log());
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A file's content at a git revision of the bare repository, or None when it is not there.
fn at(bare: &Path, rev: &str, file: &str) -> Option<String> {
    let out = Command::new("git")
        .current_dir(bare)
        .args(["show", &format!("{rev}:{file}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

#[test]
fn a_mirror_project_exports_each_landing_to_its_github_main() {
    let (scratch, hub) = hub_with_weft("github-mirror");
    let root = scratch.0.join("remotes");
    std::fs::create_dir_all(&root).unwrap();
    let (site, site_url) = remote(&root, "site");
    let (daemon, _state) = github(BTreeMap::from([("site".to_string(), site_url)]));
    let gh_host = format!("http://127.0.0.1:{}", daemon.port);

    let pid = project(&hub, "Mirror", "mirror");
    let integ = token_for(&hub, &gh_host, "site");
    let weft_repo = connect(&hub, &integ, "site", &pid);
    let change = lane(
        &hub,
        &weft_repo,
        "notes/mirror.md",
        "A mirrored note.\n",
        "Add a mirrored note",
    );
    eventually(&hub, "the lane did not land", || {
        state_of(&hub, &change) == "landed"
    });

    // The landing reaches GitHub's main as one commit, titled by its intent, fast-forward.
    eventually(&hub, "the landing was not exported", || {
        at(&site, "main", "notes/mirror.md").is_some()
    });
    assert_eq!(
        at(&site, "main", "notes/mirror.md").as_deref(),
        Some("A mirrored note.\n")
    );
    assert_eq!(
        git(&site, &["log", "-1", "--format=%s", "main"]),
        "Add a mirrored note"
    );
    assert_eq!(
        git(&site, &["rev-list", "--count", "main"]),
        "3",
        "two readmes and the landing"
    );
    assert!(
        git(&site, &["branch", "--list", "agentplaza/*"]).is_empty(),
        "a mirror project pushes no lane branches"
    );
}

#[test]
fn a_ci_project_lands_a_lane_only_once_github_merges_its_pull_request_after_its_checks_pass() {
    let (scratch, hub) = hub_with_weft("github-ci");
    let root = scratch.0.join("remotes");
    std::fs::create_dir_all(&root).unwrap();
    let (site, site_url) = remote(&root, "ci-site");
    let (daemon, state) = github(BTreeMap::from([("ci-site".to_string(), site_url)]));
    let gh_host = format!("http://127.0.0.1:{}", daemon.port);

    let pid = project(&hub, "CI", "ci");
    let integ = token_for(&hub, &gh_host, "ci-site");
    let weft_repo = connect(&hub, &integ, "ci-site", &pid);

    // The lane is pushed as its own branch and opened as a pull request into main.
    let change = lane(
        &hub,
        &weft_repo,
        "notes/ci.md",
        "A CI note.\n",
        "Add a CI note",
    );
    let branch = format!("agentplaza/{change}");
    eventually(&hub, "no pull request was opened for the lane", || {
        state
            .lock()
            .unwrap()
            .opened
            .iter()
            .any(|p| p["head"] == branch.as_str())
    });
    assert_eq!(
        at(&site, &branch, "notes/ci.md").as_deref(),
        Some("A CI note.\n"),
        "the branch holds the lane on main's head"
    );
    assert_eq!(
        git(&site, &["rev-list", "--count", &branch]),
        "3",
        "its parent is main's head"
    );

    // A failing check keeps the lane waiting, and the gate is a run of the ci recipe with the check's row.
    {
        let mut gh = state.lock().unwrap();
        gh.checks = vec![
            json!({ "name": "lint", "status": "completed", "conclusion": "success", "html_url": "" }),
            json!({ "name": "tests", "status": "completed", "conclusion": "failure", "html_url": "" }),
        ];
    }
    let github_run = |outcome: &str| -> bool {
        let (_, runs) = hub.call("GET", "/v1/recipe-runs", None);
        let list = runs
            .as_array()
            .cloned()
            .or_else(|| runs["runs"].as_array().cloned())
            .unwrap_or_default();
        list.iter().any(|r| {
            r["trigger"] == "github" && r["subject"] == change.as_str() && r["state"] == outcome
        })
    };
    eventually(&hub, "the failing check did not fail the gate", || {
        github_run("failed")
    });
    assert_eq!(
        state_of(&hub, &change),
        "queued",
        "a failed gate does not land"
    );

    // Green checks pass the gate, but a pull request nobody merged lands nothing.
    {
        let mut gh = state.lock().unwrap();
        gh.checks = vec![
            json!({ "name": "lint", "status": "completed", "conclusion": "success", "html_url": "" }),
            json!({ "name": "tests", "status": "completed", "conclusion": "success", "html_url": "" }),
        ];
    }
    eventually(&hub, "the passing checks did not pass the gate", || {
        github_run("passed")
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        state_of(&hub, &change),
        "queued",
        "GitHub has not merged it yet"
    );

    // GitHub merges it (a fast-forward of main onto the branch): Weft lands the lane, and GitHub's merge is its head.
    let merged = git(&site, &["rev-parse", &branch]);
    git(&site, &["update-ref", "refs/heads/main", &merged]);
    {
        let mut gh = state.lock().unwrap();
        for p in gh.pulls.iter_mut() {
            p["state"] = json!("closed");
            p["merged_at"] = json!("2026-10-23T10:00:00Z");
            p["merge_commit_sha"] = json!(merged);
        }
    }
    eventually(&hub, "the merged lane did not land", || {
        state_of(&hub, &change) == "landed"
    });
    // The landing is recorded before GitHub's merge commit is recorded as the connection's head (a call after the
    // landing): the head is waited for, not read as soon as the lane reads landed.
    eventually(
        &hub,
        "GitHub's merge commit was not recorded as the head",
        || {
            let (_, repo) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
            repo["connections"][0]["head"] == merged.as_str()
        },
    );
    let (_, repo) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
    assert_eq!(repo["connections"][0]["head"], merged.as_str(), "{repo}");
    assert_eq!(
        at(&site, "main", "notes/ci.md").as_deref(),
        Some("A CI note.\n")
    );

    // GitHub's main moves on its own (a commit that is not a lane): it is imported before the next lane is pushed.
    let work = root.join("ci-site-outside");
    git(
        &root,
        &[
            "clone",
            "-q",
            site.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("outside.md"), "Written on GitHub.\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Written on GitHub"]);
    git(&work, &["push", "-q", "origin", "main"]);
    let outside = git(&site, &["rev-parse", "main"]);
    let second = lane(
        &hub,
        &weft_repo,
        "notes/second.md",
        "A second note.\n",
        "Add a second note",
    );
    eventually(
        &hub,
        "the second lane was not pushed after the import",
        || {
            state
                .lock()
                .unwrap()
                .opened
                .iter()
                .any(|p| p["head"] == format!("agentplaza/{second}").as_str())
        },
    );
    let (_, repo) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
    assert_eq!(
        repo["connections"][0]["head"],
        outside.as_str(),
        "GitHub's own commit is imported: {repo}"
    );
    assert_eq!(
        at(&site, &format!("agentplaza/{second}"), "outside.md").as_deref(),
        Some("Written on GitHub.\n"),
        "the second lane's branch starts from what GitHub has"
    );
    assert_eq!(state_of(&hub, &second), "queued");
}

/// GitHub's `X-Hub-Signature-256` of a body: `sha256=` and the HMAC-SHA256 under the webhook's secret.
fn sign_push(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

#[test]
fn a_signed_push_imports_github_main_now_and_an_unsigned_one_is_refused() {
    const SECRET: &str = "push-secret-94";
    let (scratch, hub) = hub_with_weft("github-push");
    let root = scratch.0.join("remotes");
    std::fs::create_dir_all(&root).unwrap();
    let (site, site_url) = remote(&root, "ci-site");
    let (daemon, _state) = github(BTreeMap::from([("ci-site".to_string(), site_url)]));
    let gh_host = format!("http://127.0.0.1:{}", daemon.port);

    let pid = project(&hub, "CI", "ci");
    let integ = token_for(&hub, &gh_host, "ci-site");
    let weft_repo = connect(&hub, &integ, "ci-site", &pid);
    // The owner makes the connector's webhook (its path carries the token) and stores its signing secret.
    let (status, made) = hub.call(
        "POST",
        "/v1/connectors/github/webhook",
        Some(json!({ "secret": SECRET })),
    );
    assert_eq!(status, 201, "{made}\n{}", hub.log());
    let hook_path = made["path"].as_str().unwrap().to_string();

    // GitHub's main moves on its own; the push says so.
    let work = root.join("push-site-outside");
    git(
        &root,
        &[
            "clone",
            "-q",
            site.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("pushed.md"), "Pushed to GitHub.\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Pushed to GitHub"]);
    git(&work, &["push", "-q", "origin", "main"]);
    let outside = git(&site, &["rev-parse", "main"]);
    let (_, repo) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
    let before = repo["connections"][0]["head"].clone();
    assert_ne!(
        before,
        outside.as_str(),
        "the import has not seen the push yet"
    );

    // The repository's full name is its remote's last two segments.
    let remote = repo["connections"][0]["remote"]
        .as_str()
        .unwrap()
        .trim_end_matches(".git");
    let parts: Vec<&str> = remote.split('/').collect();
    let full_name = format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]);
    let body =
        json!({ "ref": "refs/heads/main", "repository": { "full_name": full_name } }).to_string();

    // A delivery that is not signed right is refused, and imports nothing.
    let (status, _) = hub.send_with_headers(
        "POST",
        &hook_path,
        &[
            ("x-hub-signature-256", "sha256=00"),
            ("x-github-event", "push"),
        ],
        body.as_bytes(),
    );
    assert_eq!(status, 401, "{}", hub.log());
    let (_, repo) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
    assert_eq!(
        repo["connections"][0]["head"], before,
        "nothing was imported"
    );

    // A signed push imports GitHub's main at once: the answer names the repo, and its head is GitHub's.
    let signature = sign_push(SECRET, body.as_bytes());
    let (status, reply) = hub.send_with_headers(
        "POST",
        &hook_path,
        &[
            ("x-hub-signature-256", &signature),
            ("x-github-event", "push"),
        ],
        body.as_bytes(),
    );
    assert_eq!(status, 202, "{reply}\n{}", hub.log());
    assert_eq!(reply["imported"], json!([weft_repo.clone()]), "{reply}");
    let (_, repo) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
    assert_eq!(repo["connections"][0]["head"], outside.as_str(), "{repo}");
}

/// Whether a GitHub-triggered run of `change` ended with `outcome`.
fn github_run(hub: &Hub, change: &str, outcome: &str) -> bool {
    let (_, runs) = hub.call("GET", "/v1/recipe-runs", None);
    let list = runs
        .as_array()
        .cloned()
        .or_else(|| runs["runs"].as_array().cloned())
        .unwrap_or_default();
    list.iter()
        .any(|r| r["trigger"] == "github" && r["subject"] == change && r["state"] == outcome)
}

/// A POST or PATCH to the fake GitHub API the way a workflow run's check reports it: the status and the JSON answer.
fn github_call(method: &str, url: &str, body: Value) -> (u16, Value) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let request = match method {
        "POST" => agent.post(url),
        _ => agent.patch(url),
    };
    let mut reply = request
        .header("Authorization", "Bearer ghp_run-site")
        .send_json(body)
        .expect("the fake GitHub answers");
    let status = reply.status().as_u16();
    let text = reply.body_mut().read_to_string().unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[test]
fn a_workflow_run_is_a_check_run_the_gate_waits_on_while_it_runs_and_reads_when_it_ends() {
    let (scratch, hub) = hub_with_weft("github-run-check");
    let root = scratch.0.join("remotes");
    std::fs::create_dir_all(&root).unwrap();
    let (site, site_url) = remote(&root, "run-site");
    let (daemon, state) = github(BTreeMap::from([("run-site".to_string(), site_url)]));
    let gh_host = format!("http://127.0.0.1:{}", daemon.port);
    let checks_url = format!("{gh_host}/api/v3/repos/JumpHash/run-site/check-runs");

    let pid = project(&hub, "Run checks", "ci");
    let integ = token_for(&hub, &gh_host, "run-site");
    let weft_repo = connect(&hub, &integ, "run-site", &pid);
    let change = lane(
        &hub,
        &weft_repo,
        "notes/run.md",
        "A run note.\n",
        "Add a run note",
    );
    let branch = format!("agentplaza/{change}");
    eventually(&hub, "no pull request was opened for the lane", || {
        state
            .lock()
            .unwrap()
            .opened
            .iter()
            .any(|p| p["head"] == branch.as_str())
    });
    let sha = git(&site, &["rev-parse", &branch]);

    // The run starts: its check is in progress, so the gate waits and records no verdict.
    let (status, started) = github_call(
        "POST",
        &checks_url,
        json!({
            "name": "plaza / build", "head_sha": sha, "status": "in_progress",
            "output": { "title": "plaza workflow build: running", "summary": "Run r1 is running." }
        }),
    );
    assert_eq!(status, 201, "{started}\n{}", hub.log());
    let id = started["id"].as_u64().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !github_run(&hub, &change, "failed") && !github_run(&hub, &change, "passed"),
        "an in-progress check is pending: {}",
        hub.log()
    );
    assert_eq!(state_of(&hub, &change), "queued");

    // The run fails: the check completes as a failure and the gate fails the lane.
    let (status, _) = github_call(
        "PATCH",
        &format!("{checks_url}/{id}"),
        json!({
            "status": "completed", "conclusion": "failure",
            "output": { "title": "plaza workflow build: failed", "summary": "Run r1 failed." }
        }),
    );
    assert_eq!(status, 200, "{}", hub.log());
    eventually(&hub, "the failed check did not fail the gate", || {
        github_run(&hub, &change, "failed")
    });
    assert_eq!(
        state_of(&hub, &change),
        "queued",
        "a failed gate does not land"
    );

    // The check passes on a later run of the same commit: the gate passes.
    let (status, _) = github_call(
        "PATCH",
        &format!("{checks_url}/{id}"),
        json!({
            "status": "completed", "conclusion": "success",
            "output": { "title": "plaza workflow build: passed", "summary": "Run r1 passed." }
        }),
    );
    assert_eq!(status, 200, "{}", hub.log());
    eventually(&hub, "the passing check did not pass the gate", || {
        github_run(&hub, &change, "passed")
    });

    let gh = state.lock().unwrap();
    assert_eq!(gh.created.len(), 1, "one check run was created");
    assert_eq!(gh.created[0]["name"], "plaza / build");
    assert_eq!(gh.created[0]["head_sha"], sha.as_str());
    assert_eq!(gh.created[0]["status"], "in_progress");
    assert_eq!(
        gh.updated
            .iter()
            .map(|u| u["body"]["conclusion"].clone())
            .collect::<Vec<_>>(),
        [json!("failure"), json!("success")]
    );
    assert!(gh.updated.iter().all(|u| u["id"] == id));
}
