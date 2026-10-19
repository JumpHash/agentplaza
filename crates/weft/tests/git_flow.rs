//! Git, one way: import a repository's history once, then export landed work as one signed commit per intent,
//! authored by the owner with no AI credits, checked by git itself. Path rules, a remote that moved on its own,
//! adopting its commits, and a signed release tag with a GitHub Release (against a stand-in API).

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use common::{Plaza, plaza};
use serde_json::{Value, json};
use weft::derived::Derived;
use weft::export::{Exporter, Keys};
use weft::keys::KeyPair;
use weft::landing::{Lander, Options};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "1700000000 +0100")
        .env("GIT_COMMITTER_DATE", "1700000000 +0100")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn land_all(p: &Plaza, derived: &Derived, lander: &Lander, repo: &str) {
    loop {
        match lander
            .step(&p.store, derived, repo, "main", Options::default())
            .unwrap()
        {
            weft::landing::Step::Landed { .. } => continue,
            weft::landing::Step::Empty => return,
            other => panic!("{other:?}"),
        }
    }
}

fn queue(p: &Plaza, repo: &str, change: &str) {
    let n = p.store.state().change(change).unwrap().round();
    p.hub_says(
        "queue.add",
        json!({ "repo": repo, "ref": "main", "change": change, "n": n }),
    )
    .unwrap();
}

/// A stand-in for GitHub's API: the user, their emails, releases and asset uploads.
fn fake_github() -> (apz_http::Handle, Arc<Mutex<Vec<(String, String, usize)>>>) {
    let seen: Arc<Mutex<Vec<(String, String, usize)>>> = Arc::default();
    let log = seen.clone();
    let server = apz_http::Server::bind(0).unwrap().limits(apz_http::Limits {
        max_head: 16 << 10,
        max_body: 16 << 20,
    });
    let handle = server
        .spawn(move |req: &apz_http::Request| {
            log.lock().unwrap().push((req.method.clone(), format!("{}{}", req.path, if req.query.is_empty() { String::new() } else { format!("?{}", req.query) }), req.body.len()));
            let port = req.header("host").and_then(|h| h.rsplit(':').next()).unwrap_or("0").to_string();
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/user") => apz_http::Response::json(200, &json!({ "login": "irfan", "id": 4242, "name": "Irfan Saric" })),
                ("GET", "/user/emails") => apz_http::Response::json(200, &json!([{ "email": "old@example.com", "primary": false, "verified": true }, { "email": "owner@example.com", "primary": true, "verified": true }])),
                ("GET", "/user/ssh_signing_keys") => apz_http::Response::json(200, &json!([])),
                ("POST", "/user/ssh_signing_keys") => apz_http::Response::json(201, &json!({ "id": 1 })),
                ("POST", p) if p.ends_with("/releases") => apz_http::Response::json(201, &json!({ "id": 7, "html_url": "https://github.com/irfan/app/releases/tag/v0.2.0", "upload_url": format!("http://127.0.0.1:{port}/uploads/7/assets{{?name,label}}") })),
                ("POST", p) if p.starts_with("/uploads/") => apz_http::Response::json(201, &json!({ "state": "uploaded" })),
                _ => apz_http::Response::error(404, "no such endpoint"),
            }
        })
        .unwrap();
    (handle, seen)
}

#[test]
fn import_once_then_export_signed_commits_per_intent() {
    if !have("git") {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("git-flow", &[]);
    p.add_packs(&["rust", "markdown"]);
    let derived = Derived::new(p.dir.join("derived"));
    let lander = Lander::default();
    let exporter = Exporter::default();
    let signing = KeyPair::generate();

    // A repository with history: an executable, a symlink, folders, an AI co-author trailer, a tag.
    let src = p.dir.join("src-repo");
    std::fs::create_dir_all(src.join("src")).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    git(&src, &["config", "user.name", "Irfan Saric"]);
    git(&src, &["config", "user.email", "owner@example.com"]);
    std::fs::write(src.join("README.md"), "# app\n").unwrap();
    std::fs::write(
        src.join("src/main.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(src.join("scripts")).unwrap();
    std::fs::write(src.join("scripts/run.sh"), "#!/bin/sh\ncargo run\n").unwrap();
    git(
        &src,
        &["update-index", "--add", "--chmod=+x", "scripts/run.sh"],
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink("README.md", src.join("LINK.md")).unwrap();
    // Without symlinks (Windows), git keeps a link as a file holding its target and the index entry's mode,
    // so the link goes into the index directly.
    #[cfg(not(unix))]
    {
        git(&src, &["config", "core.symlinks", "false"]);
        std::fs::write(src.join("LINK.md"), "README.md").unwrap();
        let blob = git(&src, &["hash-object", "-w", "LINK.md"]);
        git(
            &src,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("120000,{blob},LINK.md"),
            ],
        );
    }
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "Start the app"]);
    std::fs::write(
        src.join("src/main.rs"),
        "fn main() {\n    println!(\"hello, world\");\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/guide.md"), "# Guide\n\nRun it.\n").unwrap();
    git(&src, &["add", "-A"]);
    git(
        &src,
        &[
            "commit",
            "-q",
            "-m",
            "Say hello to the world\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
        ],
    );
    git(&src, &["rm", "-q", "scripts/run.sh"]);
    git(&src, &["commit", "-q", "-m", "Drop the run script"]);
    git(&src, &["tag", "-a", "v0.1.0", "-m", "First release"]);
    let remote = p.dir.join("irfan").join("app.git");
    std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
    git(
        &p.dir,
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    let remote_url = format!("file://{}", remote.display());

    // Connect and import, once.
    p.owner_says("repo.create", json!({ "repo": "app", "project": "pr5" }))
        .unwrap();
    p.owner_says(
        "connection.add",
        weft::connect::connection_body(
            "app",
            "origin",
            &remote_url,
            "github",
            "main",
            "Irfan Saric",
            "owner@example.com",
            Some("GITHUB_TOKEN"),
        ),
    )
    .unwrap();
    let source = weft::import::Source::open(&remote_url, None, "main").unwrap();
    let report = weft::import::import(&p.store, "app", &source, "main", "main").unwrap();
    assert_eq!(
        (report.commits, report.releases.clone(), report.checked),
        (3, vec!["v0.1.0".to_string()], 1)
    );
    {
        let st = p.store.state();
        let hello = st
            .changes
            .values()
            .find(|c| c.title == "Say hello to the world")
            .unwrap();
        assert_eq!(
            hello.import.as_ref().unwrap().ai_coauthors,
            vec!["Claude Opus 5.5 <noreply@anthropic.com>".to_string()]
        );
        assert_eq!(
            st.repos["app"].releases["v0.1.0"].propose.notes,
            "First release"
        );
    }
    let head = p.store.state().repos["app"].head("main");
    assert_eq!(
        p.store.tree(&head).unwrap().get("LINK.md").unwrap().mode,
        0o120000
    );
    // Importing again brings nothing new.
    assert_eq!(
        weft::import::import(
            &p.store,
            "app",
            &weft::import::Source::open(&remote_url, None, "main").unwrap(),
            "main",
            "main"
        )
        .unwrap()
        .commits,
        0
    );

    // The first export finds the remote at the imported head: nothing to push.
    let keys = || Keys {
        token: None,
        signing: Some(&signing),
    };
    let r = exporter
        .export(&p.store, &derived, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(r.state, "in-step", "{r:?}");

    // Priya's lane: two intents, three files.
    let main_rs = "fn main() {\n    let name = std::env::args().nth(1).unwrap_or(\"world\".into());\n    println!(\"hello, {name}\");\n}\n";
    let change = p.lane(
        "app",
        107,
        "t70",
        "Greet by name",
        &[vec![
            p.put("src/main.rs", main_rs),
            p.put("docs/guide.md", "# Guide\n\nRun it with a name.\n"),
            p.put("README.md", "# app\n\nGreets you.\n"),
        ]],
    );
    p.agent_says(107, "intent.set", json!({ "change": change, "n": 1, "intents": [
        { "title": "Greet people by name", "summary": "The first argument names who to greet.", "kind": "feature", "covers": ["src/**"] },
        { "title": "Document greeting by name", "kind": "docs", "covers": ["docs/**"] }
    ] })).unwrap();
    queue(&p, "app", &change);
    land_all(&p, &derived, &lander, "app");
    let r = exporter
        .export(&p.store, &derived, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(r.state, "pushed", "{r:?}");
    assert_eq!(r.changes[0].commits.len(), 2);

    // Git agrees: two commits by the owner, one per intent, no credits; the objects are sound; the tree is Weft's.
    let log = git(
        &remote,
        &["log", "-n", "2", "--format=%an <%ae>|%cn <%ce>|%s|%b%x00"],
    );
    let entries: Vec<&str> = log
        .split('\0')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(entries.len(), 2);
    assert!(entries[0].starts_with("Irfan Saric <owner@example.com>|Irfan Saric <owner@example.com>|Document greeting by name|"), "{}", entries[0]);
    assert!(entries[1].starts_with("Irfan Saric <owner@example.com>|Irfan Saric <owner@example.com>|Greet people by name|The first argument names who to greet."), "{}", entries[1]);
    for e in &entries {
        let lower = e.to_lowercase();
        assert!(
            !lower.contains("co-authored")
                && !lower.contains("claude")
                && !lower.contains("agent")
                && !lower.contains("priya"),
            "{e}"
        );
    }
    git(&remote, &["fsck", "--strict", "--no-dangling"]);
    let weft_head = p.store.state().repos["app"].head("main");
    weft::import::check_tree(
        &p.store,
        &weft_head,
        &weft::git::parse_oid(&git(&remote, &["rev-parse", "main^{tree}"])).unwrap(),
    )
    .unwrap();
    // README went with the last intent; the first commit has only src/main.rs.
    assert_eq!(
        git(&remote, &["show", "--name-only", "--format=", "main~1"]),
        "src/main.rs"
    );

    // The SSH signatures verify with git and ssh-keygen.
    if have("ssh-keygen") {
        let allowed = p.dir.join("allowed_signers");
        std::fs::write(
            &allowed,
            format!(
                "owner@example.com namespaces=\"git\" {}\n",
                weft::sshsig::public_line(&signing, "weft")
            ),
        )
        .unwrap();
        let out = Command::new("git")
            .args([
                "-c",
                "gpg.format=ssh",
                "-c",
                &format!("gpg.ssh.allowedSignersFile={}", allowed.display()),
                "verify-commit",
                "main",
            ])
            .current_dir(&remote)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "verify-commit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // A path the rules keep out of git.
    p.owner_says(
        "path.rule",
        json!({ "repo": "app", "glob": "secrets/**", "export": false }),
    )
    .unwrap();
    let secret = p.lane(
        "app",
        112,
        "t71",
        "Keys for the tests",
        &[vec![
            p.put("secrets/test.key", "not for git\n"),
            p.put("src/lib.rs", "pub fn id() -> u8 {\n    7\n}\n"),
        ]],
    );
    queue(&p, "app", &secret);
    land_all(&p, &derived, &lander, "app");
    assert_eq!(
        exporter
            .export(&p.store, &derived, "app", "origin", keys(), false)
            .unwrap()
            .state,
        "pushed"
    );
    let files = git(&remote, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(
        files.contains("src/lib.rs") && !files.contains("secrets"),
        "{files}"
    );

    // Someone commits on the remote directly: the export stops, nothing is forced.
    let work = p.dir.join("hotfix");
    git(
        &p.dir,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    git(&work, &["config", "user.name", "Irfan Saric"]);
    git(&work, &["config", "user.email", "owner@example.com"]);
    std::fs::write(work.join("HOTFIX.md"), "fixed on the platform\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Hotfix on the platform"]);
    git(&work, &["push", "-q", "origin", "main"]);
    let remote_head = git(&remote, &["rev-parse", "main"]);
    let docs = p.lane(
        "app",
        107,
        "t72",
        "More docs",
        &[vec![p.put("docs/more.md", "# More\n")]],
    );
    queue(&p, "app", &docs);
    land_all(&p, &derived, &lander, "app");
    let r = exporter
        .export(&p.store, &derived, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(r.state, "moved", "{r:?}");
    assert_eq!(
        git(&remote, &["rev-parse", "main"]),
        remote_head,
        "nothing was pushed"
    );
    assert_eq!(
        p.store.state().repos["app"].connections["origin"].state,
        "moved"
    );

    // The owner adopts the remote's commit: a lane, landed like any change; the export then joins the remote's head.
    let adopted = weft::import::adopt(&p.store, "app", "origin", None).unwrap();
    assert_eq!(adopted.commits, vec!["Hotfix on the platform".to_string()]);
    assert_eq!(
        exporter
            .export(&p.store, &derived, "app", "origin", keys(), false)
            .unwrap()
            .state,
        "adopting",
        "the export waits for the adopting change"
    );
    queue(&p, "app", &adopted.change);
    land_all(&p, &derived, &lander, "app");
    let r = exporter
        .export(&p.store, &derived, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(r.state, "pushed", "{r:?}");
    let merge_parents = git(&remote, &["log", "--merges", "-n", "1", "--format=%P"]);
    assert!(
        merge_parents.contains(&remote_head),
        "the merge joins the remote's own head: {merge_parents}"
    );
    // Git's tree is Weft's, less what the rules keep out.
    let weft_head = p.store.state().repos["app"].head("main");
    let mut exported = (*p.store.tree(&weft_head).unwrap()).clone();
    exported.retain(|path, _| !path.starts_with("secrets/"));
    assert_eq!(
        weft::git::hex(&weft::import::git_tree_id(&p.store, &exported).unwrap()),
        git(&remote, &["rev-parse", "main^{tree}"])
    );
    assert!(git(&remote, &["ls-tree", "-r", "--name-only", "main"]).contains("HOTFIX.md"));
    assert_eq!(
        p.store.state().repos["app"].connections["origin"].state,
        "in-step"
    );
    git(&remote, &["fsck", "--strict", "--no-dangling"]);

    // A release: an artifact with provenance, the owner's signature, a signed tag and a GitHub Release.
    let (github, seen) = fake_github();
    unsafe {
        std::env::set_var(
            "WEFT_GITHUB_API",
            format!("http://127.0.0.1:{}", github.port()),
        )
    };
    let acct = weft::connect::github_account(&weft::connect::github_api(), "token", false).unwrap();
    assert_eq!(
        (acct.name.as_str(), acct.email.as_str()),
        ("Irfan Saric", "owner@example.com")
    );
    assert_eq!(
        weft::connect::github_account(&weft::connect::github_api(), "token", true)
            .unwrap()
            .email,
        "4242+irfan@users.noreply.github.com"
    );
    let head = p.store.state().repos["app"].head("main");
    let body = weft::release::artifact_body(
        &p.store,
        "app",
        "app",
        Some("v0.2.0"),
        "linux-x86_64",
        b"\x7fELF pretend binary",
        &head,
        "node:n1/check-vm",
        "cargo build --release",
    )
    .unwrap();
    let artifact = body["id"].as_str().unwrap().to_string();
    p.hub_says("artifact.put", body.clone()).unwrap();
    assert_eq!(
        body["provenance"]["predicateType"],
        "https://slsa.dev/provenance/v1"
    );
    let proposal = weft::release::proposal(&p.store, "app", "v0.2.0", None, None).unwrap();
    let notes = proposal["notes"].as_str().unwrap().to_string();
    assert!(
        notes.contains("## Features\n\n- Greet people by name")
            && notes.contains("## Documentation")
            && notes.contains("Hotfix on the platform"),
        "{notes}"
    );
    p.hub_says("release.propose", proposal).unwrap();
    let digest = weft::release::release_digest(
        &p.store.state(),
        "app",
        "v0.2.0",
        std::slice::from_ref(&artifact),
    )
    .unwrap();
    p.owner_says(
        "release.sign",
        json!({ "repo": "app", "version": "v0.2.0", "artifacts": [artifact], "digest": digest }),
    )
    .unwrap();
    let hub_key = KeyPair::from_hex(&p.hub_seed).unwrap();
    let rel = weft::release::export_release(
        &p.store,
        "app",
        "v0.2.0",
        "origin",
        Some("token".into()),
        Some(&signing),
        Some(&hub_key),
    )
    .unwrap();
    assert_eq!(
        rel.url.as_deref(),
        Some("https://github.com/irfan/app/releases/tag/v0.2.0")
    );
    assert_eq!(
        rel.assets,
        vec![
            "app-linux-x86_64".to_string(),
            "app-linux-x86_64.intoto.jsonl".into(),
            "SHA256SUMS".into()
        ]
    );
    assert_eq!(
        git(&remote, &["rev-parse", "v0.2.0^{commit}"]),
        git(&remote, &["rev-parse", "main"])
    );
    assert!(git(&remote, &["cat-file", "-p", "v0.2.0"]).contains("Greet people by name"));
    if have("ssh-keygen") {
        let allowed = p.dir.join("allowed_signers");
        let out = Command::new("git")
            .args([
                "-c",
                "gpg.format=ssh",
                "-c",
                &format!("gpg.ssh.allowedSignersFile={}", allowed.display()),
                "verify-tag",
                "v0.2.0",
            ])
            .current_dir(&remote)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "verify-tag: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let calls: Vec<(String, String, usize)> = seen.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|(m, path, _)| m == "POST" && path == "/repos/irfan/app/releases"),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|(m, path, n)| m == "POST" && path.contains("name=SHA256SUMS") && *n > 64)
    );
    let st = p.store.state();
    assert_eq!(st.repos["app"].releases["v0.2.0"].exports.len(), 1);
    let _: Value = json!(st.repos["app"].releases["v0.2.0"].exports);
    drop(st);
    assert!(p.store.verify_deep().is_ok());
    std::fs::remove_dir_all(&p.dir).ok();
}
