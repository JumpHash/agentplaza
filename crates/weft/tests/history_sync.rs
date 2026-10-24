//! History in sync (PLAN 21.6, 22.4): every exported landing is one git commit whose tree is the tree Weft recorded
//! for it (less what the path rules keep out); a remote that was offline catches up in order; a remote's own commits
//! come in on import; and the history view reads each landing as a commit (ids, parents, author, message, records).

mod common;

use std::path::Path;
use std::process::Command;

use common::{Plaza, plaza};
use serde_json::json;
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

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A bare remote with two commits by the owner, connected to repo `app` and imported.
fn connected(p: &Plaza) -> (std::path::PathBuf, String) {
    let src = p.dir.join("src-repo");
    std::fs::create_dir_all(src.join("src")).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    git(&src, &["config", "user.name", "Irfan Saric"]);
    git(&src, &["config", "user.email", "owner@example.com"]);
    std::fs::write(src.join("README.md"), "# app\n").unwrap();
    std::fs::write(src.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "Start the app"]);
    std::fs::write(src.join("README.md"), "# app\n\nFirst words.\n").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "Write the readme"]);
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
    let url = format!("file://{}", remote.display());
    p.owner_says("repo.create", json!({ "repo": "app", "project": "pr94" }))
        .unwrap();
    p.owner_says(
        "connection.add",
        weft::connect::connection_body(
            "app",
            "origin",
            &url,
            "git",
            "main",
            "Irfan Saric",
            "owner@example.com",
            None,
        ),
    )
    .unwrap();
    let source = weft::import::Source::open(&url, None, "main").unwrap();
    let report = weft::import::import(&p.store, "app", &source, "main", "main").unwrap();
    assert_eq!(report.commits, 2);
    (remote, url)
}

fn queue(p: &Plaza, change: &str) {
    let n = p.store.state().change(change).unwrap().round();
    p.hub_says(
        "queue.add",
        json!({ "repo": "app", "ref": "main", "change": change, "n": n }),
    )
    .unwrap();
}

fn land_all(p: &Plaza, derived: &Derived, lander: &Lander) {
    loop {
        match lander
            .step(&p.store, derived, "app", "main", Options::default())
            .unwrap()
        {
            weft::landing::Step::Landed { .. } => continue,
            weft::landing::Step::Empty => return,
            other => panic!("{other:?}"),
        }
    }
}

/// A lane with one file, sealed and queued (its title is its one intent's title).
fn one_lane(p: &Plaza, task: &str, cid: u64, title: &str, path: &str, text: &str) -> String {
    let change = p.lane("app", cid, task, title, &[vec![p.put(path, text)]]);
    queue(p, &change);
    change
}

#[test]
fn every_exported_commit_has_the_tree_weft_recorded_for_its_landing() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("history-trees", &[]);
    p.add_packs(&["rust", "markdown"]);
    let derived = Derived::new(p.dir.join("derived"));
    let lander = Lander::default();
    let exporter = Exporter::default();
    let signing = KeyPair::generate();
    let (remote, _url) = connected(&p);
    p.owner_says(
        "path.rule",
        json!({ "repo": "app", "glob": "secrets/**", "export": false }),
    )
    .unwrap();
    let keys = || Keys {
        token: None,
        signing: Some(&signing),
    };

    let first = one_lane(
        &p,
        "t1",
        107,
        "Greet the reader",
        "src/greet.rs",
        "pub fn greet() {}\n",
    );
    let secret = p.lane(
        "app",
        110,
        "t2",
        "Keep a key",
        &[vec![
            p.put("secrets/k.pem", "private\n"),
            p.put("docs/k.md", "# k\n"),
        ]],
    );
    queue(&p, &secret);
    let third = one_lane(
        &p,
        "t3",
        112,
        "Document the greeting",
        "docs/greet.md",
        "# Greeting\n",
    );
    land_all(&p, &derived, &lander);
    let r = exporter
        .export(&p.store, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(r.state, "pushed", "{r:?}");
    assert_eq!(r.changes.len(), 3, "one commit per landing");
    let _ = (first, secret, third);

    // Every exported commit: its tree is Weft's tree for the landing it carries, less the excluded paths.
    let exported: Vec<(String, String)> = {
        let st = p.store.state();
        let mut v: Vec<(String, String)> = st
            .changes
            .values()
            .filter(|c| c.import.is_none())
            .filter_map(|c| {
                let commits = c.exported.get("origin")?;
                Some((commits.last()?.git.clone(), c.landed.clone()?))
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(exported.len(), 3);
    let listed = git(&remote, &["rev-list", "--reverse", "--parents", "main"]);
    let commits: Vec<&str> = listed.lines().collect();
    // The three imported commits, then three exported ones, each with one parent (a linear history).
    assert_eq!(commits.len(), 5, "{listed}");
    for line in &commits[2..] {
        assert_eq!(line.split(' ').count(), 2, "one parent each: {line}");
    }
    for (git_id, rev) in &exported {
        let mut tree = (*p.store.tree(rev).unwrap()).clone();
        tree.retain(|path, _| !path.starts_with("secrets/"));
        let want = weft::import::git_tree_id(&p.store, &tree).unwrap();
        let got = git(&remote, &["rev-parse", &format!("{git_id}^{{tree}}")]);
        assert_eq!(
            weft::git::hex(&want),
            got,
            "the tree of {git_id} is the landing's"
        );
    }
    git(&remote, &["fsck", "--strict", "--no-dangling"]);
    let ids = git(&remote, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(
        !ids.contains("secrets/") && ids.contains("docs/k.md"),
        "{ids}"
    );
}

#[test]
fn an_offline_remote_catches_up_in_order() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("history-offline", &[]);
    p.add_packs(&["rust", "markdown"]);
    let derived = Derived::new(p.dir.join("derived"));
    let lander = Lander::default();
    let exporter = Exporter::default();
    let signing = KeyPair::generate();
    let (remote, _url) = connected(&p);
    let keys = || Keys {
        token: None,
        signing: Some(&signing),
    };
    let away = p.dir.join("irfan").join("away.git");

    // The remote is offline while two landings happen: the export is refused, nothing is pushed.
    std::fs::rename(&remote, &away).unwrap();
    one_lane(
        &p,
        "t1",
        107,
        "First landing",
        "src/one.rs",
        "fn one() {}\n",
    );
    one_lane(
        &p,
        "t2",
        110,
        "Second landing",
        "src/two.rs",
        "fn two() {}\n",
    );
    land_all(&p, &derived, &lander);
    assert!(
        exporter
            .export(&p.store, "app", "origin", keys(), false)
            .is_err()
    );
    assert!(
        p.store.state().repos["app"].connections["origin"]
            .exported
            .is_empty()
    );

    // Back online: the missed landings go out, in order, in one push.
    std::fs::rename(&away, &remote).unwrap();
    let r = exporter
        .export(&p.store, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(r.state, "pushed", "{r:?}");
    let titles = git(&remote, &["log", "--reverse", "--format=%s", "main"]);
    assert_eq!(
        titles.lines().collect::<Vec<_>>(),
        [
            "Start the app",
            "Write the readme",
            "First landing",
            "Second landing"
        ]
    );

    // Nothing more to send: the next retry is a step with nothing in it.
    let again = exporter
        .export(&p.store, "app", "origin", keys(), false)
        .unwrap();
    assert_eq!(again.state, "in-step");
}

#[test]
fn a_remote_commit_that_arrives_later_is_imported_again() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("history-import", &[]);
    let (remote, url) = connected(&p);
    let work = p.dir.join("work");
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
    std::fs::write(work.join("CHANGELOG.md"), "# Changes\n").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Start a changelog"]);
    git(&work, &["push", "-q", "origin", "main"]);

    let report = weft::import::import(
        &p.store,
        "app",
        &weft::import::Source::open(&url, None, "main").unwrap(),
        "main",
        "main",
    )
    .unwrap();
    assert_eq!(report.commits, 1, "only the new commit comes in");
    let view = weft::history::view(&p.store, "app", "main").unwrap();
    let landings = view["landings"].as_array().unwrap();
    assert_eq!(landings.len(), 3);
    assert_eq!(landings[2]["title"], "Start a changelog");
    assert_eq!(landings[2]["imported"], true);
}

#[test]
fn the_history_reads_each_landing_as_a_commit() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("history-view", &[]);
    p.add_packs(&["rust", "markdown"]);
    let derived = Derived::new(p.dir.join("derived"));
    let lander = Lander::default();
    let exporter = Exporter::default();
    let signing = KeyPair::generate();
    let (remote, _url) = connected(&p);
    let keys = || Keys {
        token: None,
        signing: Some(&signing),
    };
    let change = p.lane(
        "app",
        107,
        "t1",
        "Greet the reader",
        &[vec![p.put("src/greet.rs", "pub fn greet() {}\n")]],
    );
    p.agent_says(107, "intent.set", json!({ "change": change, "n": 1, "intents": [
        { "title": "Greet the reader", "summary": "A function for it.", "kind": "feature", "covers": ["src/**"] }
    ] })).unwrap();
    queue(&p, &change);
    // An open lane shows as a lane on the reference.
    p.lane(
        "app",
        110,
        "t2",
        "Still open",
        &[vec![p.put("docs/open.md", "# open\n")]],
    );
    land_all(&p, &derived, &lander);
    exporter
        .export(&p.store, "app", "origin", keys(), false)
        .unwrap();

    let view = weft::history::view(&p.store, "app", "main").unwrap();
    let landings = view["landings"].as_array().unwrap();
    assert_eq!(landings.len(), 3);
    let ids: std::collections::HashSet<&str> =
        landings.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 3, "content ids are unique");
    assert!(landings[0]["parents"].as_array().unwrap().is_empty());
    assert_eq!(landings[1]["parents"][0], landings[0]["id"]);
    assert_eq!(landings[2]["parents"][0], landings[1]["id"]);
    let last = &landings[2];
    let git_id = last["exported"]["origin"].as_str().unwrap();
    assert_eq!(git(&remote, &["rev-parse", "main"]), git_id);
    assert_eq!(last["author"]["email"], "owner@example.com");
    assert_eq!(
        last["message"].as_str().unwrap().trim_end(),
        git(&remote, &["log", "-n", "1", "--format=%B"])
    );
    assert!(
        !last["records"].as_array().unwrap().is_empty(),
        "the sign-off chain is there"
    );
    let lanes = view["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0]["title"], "Still open");
    // The lane was opened before the change landed: it starts from the landing before it.
    assert_eq!(lanes[0]["base"], landings[1]["id"]);
    std::fs::remove_dir_all(&p.dir).ok();
}

#[test]
fn a_landing_names_the_review_that_checked_it() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("history-review", &[]);
    p.add_packs(&["rust", "markdown"]);
    let derived = Derived::new(p.dir.join("derived"));
    let lander = Lander::default();
    let exporter = Exporter::default();
    let signing = KeyPair::generate();
    let (remote, _url) = connected(&p);
    let keys = || Keys {
        token: None,
        signing: Some(&signing),
    };
    let change = one_lane(
        &p,
        "t1",
        107,
        "Greet the reader",
        "src/greet.rs",
        "pub fn greet() {}\n",
    );
    land_all(&p, &derived, &lander);

    // The hub signs the gate of the review that checked the landed revision.
    let (rev, digest) = {
        let st = p.store.state();
        let landed = st.change(&change).unwrap().landed.clone().unwrap();
        (landed.clone(), st.revisions[&landed].digest)
    };
    p.hub_says(
        "attest",
        json!({ "revision": rev, "digest": digest, "predicate_type": "gate", "predicate": { "run": "rv7", "outcome": "passed" } }),
    )
    .unwrap();
    exporter
        .export(&p.store, "app", "origin", keys(), false)
        .unwrap();

    let message = git(&remote, &["log", "-n", "1", "--format=%B"]);
    assert!(message.lines().any(|l| l == "Review-Run: rv7"), "{message}");
    let view = weft::history::view(&p.store, "app", "main").unwrap();
    let landings = view["landings"].as_array().unwrap();
    assert_eq!(landings[2]["review_run"], "rv7");
    assert!(
        landings[0]["review_run"].is_null(),
        "an imported landing was not reviewed here"
    );
}

#[test]
fn a_release_note_edit_is_a_statement_that_survives_a_replay() {
    if !have_git() {
        eprintln!("git is not installed: skipped");
        return;
    }
    let p = plaza("history-notes", &[]);
    connected(&p);
    let proposal = weft::release::proposal(&p.store, "app", "v0.1.0", None, None).unwrap();
    p.hub_says("release.propose", proposal).unwrap();
    p.hub_says(
        "release.notes",
        json!({ "repo": "app", "version": "v0.1.0", "notes": "Edited by the owner.\n" }),
    )
    .unwrap();
    assert_eq!(
        p.store.state().repos["app"].releases["v0.1.0"]
            .propose
            .notes,
        "Edited by the owner.\n"
    );
    // A replay of the log (the store opened again from its folder) has the edit too.
    let dir = p.dir.clone();
    let hub = KeyPair::from_hex(&p.hub_seed).unwrap();
    drop(p);
    let reopened = weft::store::Store::open(&dir, Some(hub)).unwrap();
    assert_eq!(
        reopened.state().repos["app"].releases["v0.1.0"]
            .propose
            .notes,
        "Edited by the owner.\n"
    );
    reopened.verify_deep().unwrap();
    std::fs::remove_dir_all(&dir).ok();
}
