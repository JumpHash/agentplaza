//! Git platforms end to end on a scratch hub, with stand-ins for GitHub, GitLab and Forgejo (their APIs and OAuth
//! endpoints) and real git repositories behind their clone URLs:
//!
//! - GitHub at the organisation level: the plaza's app is created from its manifest, installed on an organisation
//!   (the owner signs in on the way), what GitHub gave it is what the plaza may use; a repository is connected to a
//!   project (history imported, the hub's signing key registered on the owner's account), and the owner's next edit
//!   is exported to it, by the owner as GitHub knows them.
//! - GitLab at the user level: an OAuth app the owner registered, a sign-in with PKCE; nothing may be used until the
//!   owner grants one repository of a group, which then connects to a second project.
//! - Forgejo at the repository level: a token for one repository reaches that one only.
//! - A used or unknown state does nothing; disconnecting and removing undo what was done.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use common::{FakeDaemon, Hub, Scratch, text};

const KEY: &str = include_str!("../src/platforms/testdata/github-app-test-key.pem");

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

/// A bare repository with `commits` commits on main (each adds one file); its path as a file URL.
fn remote(root: &Path, name: &str, commits: usize) -> (PathBuf, String) {
    let work = root.join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    for i in 0..commits {
        std::fs::write(
            work.join(format!("note-{i}.md")),
            format!("# Note {i}\n\nSome words.\n"),
        )
        .unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", &format!("Note {i}")]);
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

/// A query string's fields (`a=1&b=x%20y`), and a form body's.
fn fields(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace("%20", " ").replace('+', " ")))
        .collect()
}

fn field(raw: &str, name: &str) -> Option<String> {
    fields(raw)
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
}

/// The value of `name` in a URL's query.
fn query(url: &str, name: &str) -> Option<String> {
    field(url.split_once('?')?.1, name)
}

/// A browser following the hub's page: the status and the `Location` of a redirect (or the page's text).
fn browse(url: &str) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .build(),
    );
    let mut r = agent.get(url).call().expect("the hub answers");
    let status = r.status().as_u16();
    let location = r
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = r.body_mut().read_to_string().unwrap_or_default();
    (status, location.unwrap_or(body))
}

#[test]
fn the_owner_authorizes_github_gitlab_and_forgejo_and_connects_what_they_chose() {
    let scratch = Scratch::new("platforms");
    let root = scratch.0.join("remotes");
    std::fs::create_dir_all(&root).unwrap();
    let (site_bare, site_url) = remote(&root, "site", 2);
    let (_, docs_url) = remote(&root, "docs", 1);
    let (_, wiki_url) = remote(&root, "wiki", 1);

    // ── the stand-ins ──
    let gh_seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = gh_seen.clone();
    let site = site_url.clone();
    let github = FakeDaemon::serve(move |method, path, body| {
        seen.lock().unwrap().push(format!("{method} {path}"));
        let repo = json!({ "full_name": "JumpHash/site", "owner": { "login": "JumpHash", "type": "Organization", "id": 9 }, "private": true, "default_branch": "main", "clone_url": site, "html_url": "https://github.example/JumpHash/site" });
        let (status, v) = match (method, path.split('?').next().unwrap()) {
            ("POST", "/api/v3/app-manifests/mcode/conversions") => (
                201,
                json!({ "id": 42, "slug": "agentplaza-test", "name": "AgentPlaza test", "owner": { "login": "irfan" }, "client_id": "Iv23test", "client_secret": "app-secret", "pem": KEY, "html_url": "https://github.example/apps/agentplaza-test" }),
            ),
            ("POST", "/login/oauth/access_token") if body.contains("\"code\":\"ucode\"") => (
                200,
                json!({ "access_token": "ghu_owner", "expires_in": 28800, "refresh_token": "ghr_owner", "token_type": "bearer", "scope": "" }),
            ),
            ("GET", "/api/v3/user") => (
                200,
                json!({ "id": 5, "login": "irfan", "name": "Irfan S", "avatar_url": "" }),
            ),
            ("GET", "/api/v3/user/emails") => (
                200,
                json!([{ "email": "irfan@x.test", "verified": true, "primary": true }]),
            ),
            ("GET", "/api/v3/app/installations") => (
                200,
                json!([{ "id": 7, "account": { "login": "JumpHash", "type": "Organization", "id": 9 }, "repository_selection": "selected", "html_url": "https://github.example/organizations/JumpHash/settings/installations/7" }]),
            ),
            ("POST", "/api/v3/app/installations/7/access_tokens") => (
                201,
                json!({ "token": "ghs_inst", "expires_at": "2099-01-01T00:00:00Z" }),
            ),
            ("GET", "/api/v3/installation/repositories") => {
                (200, json!({ "total_count": 1, "repositories": [repo] }))
            }
            ("GET", "/api/v3/repos/JumpHash/site") => (200, repo),
            ("POST", "/api/v3/user/ssh_signing_keys") => (201, json!({ "id": 1 })),
            _ => (404, json!({ "message": "Not Found" })),
        };
        (status, v.to_string())
    });
    let gh_host = format!("http://127.0.0.1:{}", github.port);

    let gl_seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let seen = gl_seen.clone();
    let docs = docs_url.clone();
    let gitlab = FakeDaemon::serve(move |method, path, body| {
        seen.lock()
            .unwrap()
            .push((format!("{method} {path}"), body.to_string()));
        let project = json!({ "id": 31, "path_with_namespace": "team/tools/docs", "path": "docs", "namespace": { "full_path": "team/tools" }, "visibility": "private", "default_branch": "main", "http_url_to_repo": docs, "web_url": "https://gitlab.example/team/tools/docs", "permissions": { "project_access": { "access_level": 40 } } });
        let (status, v) = match (method, path.split('?').next().unwrap()) {
            ("POST", "/oauth/token")
                if field(body, "code").as_deref() == Some("glcode")
                    && field(body, "code_verifier").is_some() =>
            {
                (
                    200,
                    json!({ "access_token": "glat", "token_type": "Bearer", "expires_in": 7200, "refresh_token": "glrt", "scope": "api read_user", "created_at": 1 }),
                )
            }
            ("GET", "/api/v4/user") => (
                200,
                json!({ "id": 11, "username": "irfan", "name": "Irfan S", "email": "irfan@gl.test", "confirmed_at": "2026-01-01T00:00:00Z", "avatar_url": "" }),
            ),
            ("GET", "/api/v4/groups") => (
                200,
                json!([{ "id": 21, "full_path": "team/tools", "name": "tools", "full_name": "Team / Tools" }]),
            ),
            ("GET", "/api/v4/groups/21/projects") => (
                200,
                json!([project, { "id": 32, "path_with_namespace": "team/tools/secret", "path": "secret", "namespace": { "full_path": "team/tools" }, "visibility": "private", "default_branch": "main", "http_url_to_repo": "https://gitlab.example/team/tools/secret.git", "web_url": "" }]),
            ),
            ("GET", "/api/v4/users/11/projects") => (200, json!([])),
            ("GET", "/api/v4/projects/team%2Ftools%2Fdocs") => (200, project),
            _ => (404, json!({ "message": "404 Not Found" })),
        };
        (status, v.to_string())
    });
    let gl_host = format!("http://127.0.0.1:{}", gitlab.port);

    let wiki = wiki_url.clone();
    let forgejo = FakeDaemon::serve(move |method, path, _| {
        let repo = json!({ "id": 3, "full_name": "irfan/wiki", "name": "wiki", "owner": { "login": "irfan" }, "private": false, "default_branch": "main", "clone_url": wiki, "html_url": "https://codeberg.example/irfan/wiki", "permissions": { "admin": true, "push": true, "pull": true } });
        let (status, v) = match (method, path.split('?').next().unwrap()) {
            ("GET", "/api/v1/user") => (
                200,
                json!({ "id": 3, "login": "irfan", "full_name": "Irfan S", "email": "irfan@fj.test", "avatar_url": "" }),
            ),
            ("GET", "/api/v1/user/emails") => (
                200,
                json!([{ "email": "irfan@fj.test", "verified": true, "primary": true }]),
            ),
            ("GET", "/api/v1/user/orgs") => (
                200,
                json!([{ "id": 4, "username": "club", "full_name": "The Club" }]),
            ),
            ("GET", "/api/v1/repos/irfan/wiki") => (200, repo),
            ("GET", "/api/v1/user/repos") => (200, json!([repo])),
            _ => (404, json!({ "message": "not found" })),
        };
        (status, v.to_string())
    });
    let fj_host = format!("http://127.0.0.1:{}", forgejo.port);

    let hub = Hub::start(&scratch, None, None);
    let weft = hub.wait_for("/v1/weft", Duration::from_secs(30), |w| {
        w["running"] == true
    });
    assert_eq!(weft["running"], true, "{weft}\n{}", hub.log());
    let project = |title: &str| {
        let (status, p) = hub.call(
            "POST",
            "/v1/projects",
            Some(json!({ "title": title, "goal": title, "kind": "other" })),
        );
        assert_eq!(status, 201, "{p}");
        text(&p["id"])
    };
    let site_project = project("Site");
    let docs_project = project("Docs");

    // ── GitHub: the app from its manifest, installed on an organisation ──
    let (status, started) = hub.call(
        "POST",
        "/v1/platforms/github/app",
        Some(json!({ "host": gh_host, "name": "AgentPlaza test" })),
    );
    assert_eq!(status, 200, "{started}");
    let state = text(&started["state"]);
    let (status, page) = browse(&text(&started["go"]));
    assert_eq!(status, 200);
    assert!(
        page.contains(&format!("{gh_host}/settings/apps/new?state={state}")),
        "{page}"
    );
    assert!(
        page.contains("&quot;request_oauth_on_install&quot;:true"),
        "the manifest rides in the form: {page}"
    );
    let (status, flow) = hub.call("GET", &format!("/v1/platforms/flows/{state}"), None);
    assert_eq!(
        (status, text(&flow["status"])),
        (200, "pending".to_string())
    );

    // GitHub sends the browser back with the manifest's code: the app is created, and the browser goes on to install.
    let callback = |q: &str| {
        browse(&format!(
            "http://127.0.0.1:{}/v1/platforms/callback?{q}",
            hub.port
        ))
    };
    let (status, install_go) = callback(&format!("code=mcode&state={state}"));
    assert_eq!(status, 302, "{install_go}\n{}", hub.log());
    let (status, install_page) = browse(&install_go);
    assert_eq!(status, 302);
    assert!(
        install_page.starts_with(&format!(
            "{gh_host}/apps/agentplaza-test/installations/new?state="
        )),
        "{install_page}"
    );
    let install_state = query(&install_page, "state").unwrap();
    // The owner installs it on JumpHash and picks its repositories there; GitHub signs them in on the way back.
    let (status, done) = callback(&format!(
        "code=ucode&installation_id=7&setup_action=install&state={install_state}"
    ));
    assert_eq!(status, 200, "{done}\n{}", hub.log());
    assert!(done.contains("installed"), "{done}");
    let (_, flow) = hub.call("GET", &format!("/v1/platforms/flows/{state}"), None);
    assert_eq!(
        flow["status"], "done",
        "the first state reads as the whole thing: {flow}"
    );
    // The same state twice does nothing.
    let (status, _) = callback(&format!("code=mcode&state={state}"));
    assert_eq!(status, 400);
    let (status, _) = callback("code=evil&state=unknown");
    assert_eq!(status, 400);

    let (_, list) = hub.call("GET", "/v1/platforms", None);
    let integrations = list["integrations"].as_array().unwrap().clone();
    let installation = integrations
        .iter()
        .find(|i| i["via"] == "app")
        .expect("the installation")
        .clone();
    let signed_in = integrations
        .iter()
        .find(|i| i["via"] == "oauth")
        .expect("the owner's sign-in")
        .clone();
    assert_eq!(
        (
            installation["level"].clone(),
            installation["target"].clone()
        ),
        (json!("org"), json!("JumpHash")),
        "{installation}"
    );
    assert_eq!(installation["repository_selection"], "selected");
    assert_eq!(
        installation["grants"]["namespaces"]["JumpHash"]["all"], true,
        "what GitHub gave is what the plaza may use"
    );
    assert_eq!(
        installation["identity"]["email"], "irfan@x.test",
        "exports are by the owner: {installation}"
    );
    assert_eq!(signed_in["account"]["login"], "irfan");
    assert!(
        !list.to_string().contains("app-secret") && !list.to_string().contains("ghu_owner"),
        "no secret leaves the hub"
    );
    assert_eq!(list["registrations"][0]["app"]["slug"], "agentplaza-test");
    let inst = text(&installation["id"]);

    let (status, namespaces) = hub.call("GET", &format!("/v1/platforms/{inst}/namespaces"), None);
    assert_eq!(status, 200, "{namespaces}");
    assert_eq!(namespaces[0]["name"], "JumpHash");
    let (status, repos) = hub.call(
        "GET",
        &format!("/v1/platforms/{inst}/repos?namespace=JumpHash"),
        None,
    );
    assert_eq!(status, 200, "{repos}");
    assert_eq!(
        (repos[0]["full_name"].clone(), repos[0]["granted"].clone()),
        (json!("JumpHash/site"), json!(true))
    );

    let (status, connected) = hub.call(
        "POST",
        &format!("/v1/platforms/{inst}/connect"),
        Some(json!({ "repo": "JumpHash/site", "project": site_project, "sign": true })),
    );
    assert_eq!(status, 200, "{connected}\n{}", hub.log());
    assert_eq!(connected["imported"]["commits"], 2, "{connected}");
    assert_eq!(connected["signing_key"], "registered", "{connected}");
    assert_eq!(connected["author"], "Irfan S <irfan@x.test>");
    let weft_repo = text(&connected["weft_repo"]);
    let (_, detail) = hub.call("GET", &format!("/v1/weft/repos/{weft_repo}"), None);
    assert_eq!(
        detail["connections"][0]["credential"],
        json!(format!("platform:{inst}")),
        "{detail}"
    );
    let (status, again) = hub.call(
        "POST",
        &format!("/v1/platforms/{inst}/connect"),
        Some(json!({ "repo": "JumpHash/site", "project": site_project })),
    );
    assert_eq!(status, 409, "{again}");
    let (_, repos) = hub.call(
        "GET",
        &format!("/v1/platforms/{inst}/repos?namespace=JumpHash"),
        None,
    );
    assert_eq!(
        repos[0]["connections"][0]["weft_repo"],
        json!(weft_repo),
        "{repos}"
    );
    // The signing key went on the owner's own account, with their sign-in.
    assert!(
        gh_seen
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == "POST /api/v3/user/ssh_signing_keys")
    );

    // The owner's next edit lands and goes out to GitHub's repository, by the owner, through the installation.
    let (status, edit) = hub.call("POST", &format!("/v1/weft/repos/{weft_repo}/edits"), Some(json!({ "title": "Add the roadmap", "files": [{ "path": "ROADMAP.md", "text": "# Roadmap\n" }] })));
    assert_eq!(status, 200, "{edit}");
    let exported = hub.wait_for(
        &format!("/v1/weft/repos/{weft_repo}"),
        Duration::from_secs(20),
        |r| r["connections"][0]["exported"].as_u64().unwrap_or(0) >= 1,
    );
    assert!(
        exported["connections"][0]["exported"].as_u64().unwrap_or(0) >= 1,
        "{exported}\n{}",
        hub.log()
    );
    // The export records where it began (the imported commit) before it pushes, so `exported` can count 1 while
    // the push is still going: wait for the commit itself on the remote.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut log = String::new();
    while std::time::Instant::now() < deadline {
        log = git(
            &site_bare,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%s"],
        );
        if log.ends_with("|Add the roadmap") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        log,
        "Irfan S <irfan@x.test>|Irfan S <irfan@x.test>|Add the roadmap"
    );
    assert!(
        gh_seen
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == "POST /api/v3/app/installations/7/access_tokens"),
        "the push took an installation token"
    );

    // ── GitLab: an OAuth app the owner registered, a sign-in with PKCE ──
    let (status, reg) = hub.call("POST", "/v1/platforms/registrations", Some(json!({ "kind": "gitlab", "host": gl_host, "client_id": "gl-client", "client_secret": "gl-secret" })));
    assert_eq!(status, 200, "{reg}");
    let (status, started) = hub.call(
        "POST",
        "/v1/platforms/authorize",
        Some(json!({ "registration": reg["id"] })),
    );
    assert_eq!(status, 200, "{started}");
    let (status, authorize) = browse(&text(&started["go"]));
    assert_eq!(status, 302);
    assert!(
        authorize.starts_with(&format!("{gl_host}/oauth/authorize?client_id=gl-client")),
        "{authorize}"
    );
    assert!(
        authorize.contains("code_challenge_method=S256")
            && authorize.contains("scope=api%20read_user"),
        "{authorize}"
    );
    let gl_state = query(&authorize, "state").unwrap();
    let (status, page) = callback(&format!("code=glcode&state={gl_state}"));
    assert_eq!(status, 200, "{page}\n{}", hub.log());
    let (_, flow) = hub.call(
        "GET",
        &format!("/v1/platforms/flows/{}", text(&started["state"])),
        None,
    );
    assert_eq!(flow["status"], "done", "{flow}");
    let gl = text(&flow["detail"]["integrations"][0]);
    let (_, list) = hub.call("GET", "/v1/platforms", None);
    let gl_integ = list["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == gl.as_str())
        .unwrap()
        .clone();
    assert_eq!(
        (
            gl_integ["level"].clone(),
            gl_integ["account"]["email"].clone()
        ),
        (json!("user"), json!("irfan@gl.test"))
    );
    assert!(
        gl_integ["grants"]["namespaces"]
            .as_object()
            .unwrap()
            .is_empty(),
        "a sign-in may use nothing until the owner says"
    );
    let (_, namespaces) = hub.call("GET", &format!("/v1/platforms/{gl}/namespaces"), None);
    let names: Vec<String> = namespaces
        .as_array()
        .unwrap()
        .iter()
        .map(|n| text(&n["name"]))
        .collect();
    assert_eq!(names, ["irfan", "team/tools"]);
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/platforms/{gl}/connect"),
        Some(json!({ "repo": "team/tools/docs", "project": docs_project })),
    );
    assert_eq!(status, 403, "{refused}");
    let (status, _) = hub.call(
        "PUT",
        &format!("/v1/platforms/{gl}/grants"),
        Some(json!({ "namespaces": { "team/tools": { "all": false, "repos": ["docs"] } } })),
    );
    assert_eq!(status, 200);
    let (_, repos) = hub.call(
        "GET",
        &format!("/v1/platforms/{gl}/repos?namespace=team%2Ftools"),
        None,
    );
    let granted: Vec<(String, bool)> = repos
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (text(&r["name"]), r["granted"] == true))
        .collect();
    assert_eq!(
        granted,
        [("docs".to_string(), true), ("secret".to_string(), false)]
    );
    let (status, connected) = hub.call(
        "POST",
        &format!("/v1/platforms/{gl}/connect"),
        Some(json!({ "repo": "team/tools/docs", "project": docs_project })),
    );
    assert_eq!(status, 200, "{connected}\n{}", hub.log());
    assert_eq!(
        (
            connected["imported"]["commits"].clone(),
            connected["connection"].clone()
        ),
        (json!(1), json!("gitlab"))
    );
    assert!(
        gl_seen
            .lock()
            .unwrap()
            .iter()
            .any(|(c, b)| c == "POST /oauth/token" && b.contains("code_verifier=")),
        "PKCE"
    );

    // ── Forgejo: a token for one repository reaches that one only ──
    let (status, fj) = hub.call("POST", "/v1/platforms/tokens", Some(json!({ "kind": "forgejo", "host": fj_host, "token": "fj-token", "level": "repo", "target": "irfan/wiki" })));
    assert_eq!(status, 200, "{fj}");
    assert_eq!(
        fj["grants"]["namespaces"]["irfan"]["repos"],
        json!(["wiki"])
    );
    let fj_id = text(&fj["id"]);
    let (_, namespaces) = hub.call("GET", &format!("/v1/platforms/{fj_id}/namespaces"), None);
    assert_eq!(
        namespaces.as_array().unwrap().len(),
        1,
        "a repository-level token reaches its namespace only: {namespaces}"
    );
    let (_, repos) = hub.call(
        "GET",
        &format!("/v1/platforms/{fj_id}/repos?namespace=irfan"),
        None,
    );
    assert_eq!(repos.as_array().unwrap().len(), 1);
    assert_eq!(repos[0]["full_name"], "irfan/wiki");
    let (status, bad) = hub.call(
        "POST",
        "/v1/platforms/tokens",
        Some(json!({ "kind": "forgejo", "host": fj_host, "token": "t", "level": "org" })),
    );
    assert_eq!(
        status, 400,
        "an organisation-level token names its organisation: {bad}"
    );

    // ── undoing ──
    let (status, _) = hub.call(
        "POST",
        &format!("/v1/platforms/{gl}/disconnect"),
        Some(json!({ "weft_repo": connected["weft_repo"], "connection": "gitlab" })),
    );
    assert_eq!(status, 200);
    let (_, detail) = hub.call(
        "GET",
        &format!("/v1/weft/repos/{}", text(&connected["weft_repo"])),
        None,
    );
    assert!(
        detail["connections"].as_array().unwrap().is_empty(),
        "{detail}"
    );
    let (status, _) = hub.call("DELETE", &format!("/v1/platforms/{fj_id}"), None);
    assert_eq!(status, 200);
    let (_, list) = hub.call("GET", "/v1/platforms", None);
    assert!(
        !list["integrations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == fj_id.as_str())
    );
}
