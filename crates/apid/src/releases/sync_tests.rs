//! The release sync against stand-in platforms (PLAN 22.4): a release appears on GitHub, GitLab and Forgejo with its
//! assets, its SBOM and provenance, `SHA256SUMS` and its verdicts as checks; a candidate is a pre-release on a channel
//! that is not stable; an edited note and a yank sync to the release; an artifact too large to attach is linked.

use std::sync::Arc;

use serde_json::{Value, json};

use super::entity::{Release, ReleaseArtifact, ReleaseState, ReleaseVerdict};
use super::sync::{Input, Target, key_of, sync_one};
use crate::platforms::forge::Repo;
use crate::platforms::http::{Body, Fake};
use crate::platforms::model::Kind;

const DIGEST: &str = "d1";

fn release(state: ReleaseState, channel: &str, notes: &str) -> Release {
    Release {
        id: "rel-1".into(),
        project: "pr5".into(),
        repo: "app".into(),
        version: "v0.2.0".into(),
        channel: channel.into(),
        base: None,
        lane: "main".into(),
        from: "w-1.l".into(),
        state,
        number: None,
        notes: notes.into(),
        run: None,
        changes: vec![],
        artifacts: vec![ReleaseArtifact {
            target: "cargo:app@linux-x86_64".into(),
            artifact: "a-1".into(),
            digest: DIGEST.into(),
            size: 10,
            signatures: String::new(),
            sbom: "{\"bomFormat\":\"CycloneDX\"}".into(),
            provenance: "{\"payloadType\":\"application/vnd.in-toto+json\"}".into(),
            pool: String::new(),
            base: String::new(),
            run: String::new(),
        }],
        not_run: vec![],
        verdicts: vec![ReleaseVerdict {
            source: "ci".into(),
            verdict: "passed".into(),
            reason: String::new(),
            seq: 5,
        }],
        promotions: vec![],
        sync: vec![],
        created_ms: 0,
        updated_ms: 0,
    }
}

fn bytes(_: &str) -> Result<Vec<u8>, String> {
    Ok(b"\x7fELF pretend binary".to_vec())
}

fn sync(
    fake: &Arc<Fake>,
    kind: Kind,
    host: &str,
    path: &str,
    rel: &Release,
    limit: u64,
    link: Option<&str>,
) -> Result<Vec<crate::releases::entity::ReleaseSync>, String> {
    let read = |d: &str| bytes(d);
    let input = Input {
        release: rel,
        commit: "c0ffee",
        bytes: &read,
        link_base: link,
        asset_limit: limit,
    };
    let target = Target {
        kind,
        host: host.into(),
        repo: Repo::new(path),
        token: "token".into(),
    };
    sync_one(fake.clone(), &input, &target, 1000)
}

/// The body of the last call to a method and a URL's path (its query aside).
fn body_of(fake: &Fake, method: &str, path: &str) -> Value {
    fake.seen()
        .into_iter()
        .rev()
        .find(|(m, u, _, _)| m == method && u.split('?').next() == Some(path))
        .and_then(|(_, _, _, b)| match b {
            Some(Body::Json(v)) => Some(v),
            _ => None,
        })
        .unwrap_or(Value::Null)
}

fn github_stand_in(existing: bool) -> Arc<Fake> {
    Arc::new(Fake::new(move |method, url, _| match (method, url) {
        ("GET", u) if u.contains("/releases/tags/") => {
            if existing {
                (
                    200,
                    json!({ "id": 7, "html_url": "https://github.com/irfan/app/releases/tag/v0.2.0", "upload_url": "https://uploads.github.com/repos/irfan/app/releases/7/assets{?name,label}" }),
                )
            } else {
                (404, json!({ "message": "Not Found" }))
            }
        }
        ("POST", "https://api.github.com/repos/irfan/app/releases") => (
            201,
            json!({ "id": 7, "html_url": "https://github.com/irfan/app/releases/tag/v0.2.0", "upload_url": "https://uploads.github.com/repos/irfan/app/releases/7/assets{?name,label}" }),
        ),
        ("PATCH", _) => (200, json!({ "id": 7 })),
        ("POST", u) if u.starts_with("https://uploads.github.com/") => (
            201,
            json!({ "browser_download_url": "https://github.com/irfan/app/releases/download/v0.2.0/x" }),
        ),
        ("POST", "https://api.github.com/repos/irfan/app/check-runs") => (201, json!({ "id": 9 })),
        _ => (404, json!({ "message": "no such endpoint" })),
    }))
}

#[test]
fn a_release_appears_on_github_with_its_assets_checks_and_provenance() {
    let fake = github_stand_in(false);
    let rel = release(
        ReleaseState::Published,
        "stable",
        "## Features\n\n- Greet people\n",
    );
    let rows = sync(
        &fake,
        Kind::Github,
        "https://github.com",
        "irfan/app",
        &rel,
        1 << 20,
        None,
    )
    .unwrap();
    assert_eq!(rows[0].platform, "github");
    assert_eq!(rows[0].state, "published");
    assert_eq!(
        rows[0].url.as_deref(),
        Some("https://github.com/irfan/app/releases/tag/v0.2.0")
    );
    assert_eq!(rows[1].platform, "github:check");
    assert_eq!(rows[1].state, "5:passed");

    let created = body_of(
        &fake,
        "POST",
        "https://api.github.com/repos/irfan/app/releases",
    );
    assert_eq!(created["tag_name"], "v0.2.0");
    assert_eq!(created["prerelease"], false);
    assert!(created["body"].as_str().unwrap().contains("- Greet people"));
    let urls = fake.urls();
    for name in [
        "cargo-app-linux-x86_64",
        "cargo-app-linux-x86_64.cdx.json",
        "cargo-app-linux-x86_64.intoto.jsonl",
        "SHA256SUMS",
    ] {
        let want = format!(
            "POST https://uploads.github.com/repos/irfan/app/releases/7/assets?name={name}"
        );
        assert!(urls.contains(&want), "{want} in {urls:?}");
    }
    let check = body_of(
        &fake,
        "POST",
        "https://api.github.com/repos/irfan/app/check-runs",
    );
    assert_eq!(check["conclusion"], "success");
    assert_eq!(check["head_sha"], "c0ffee");
    assert_eq!(check["name"], "agentplaza/ci");
}

#[test]
fn a_candidate_is_a_pre_release_where_its_channel_is_not_stable_and_absent_where_it_is() {
    let fake = github_stand_in(false);
    let beta = release(ReleaseState::Candidate, "beta", "notes\n");
    let rows = sync(
        &fake,
        Kind::Github,
        "https://github.com",
        "irfan/app",
        &beta,
        1 << 20,
        None,
    )
    .unwrap();
    assert_eq!(rows[0].state, "pre-release");
    assert_eq!(
        body_of(
            &fake,
            "POST",
            "https://api.github.com/repos/irfan/app/releases"
        )["prerelease"],
        true
    );

    let stable = release(ReleaseState::Candidate, "stable", "notes\n");
    let fake = github_stand_in(false);
    assert!(
        sync(
            &fake,
            Kind::Github,
            "https://github.com",
            "irfan/app",
            &stable,
            1 << 20,
            None
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        fake.seen().is_empty(),
        "a stable candidate is not on the platform until it is published"
    );

    let draft = release(ReleaseState::Draft, "beta", "notes\n");
    assert!(
        sync(
            &fake,
            Kind::Github,
            "https://github.com",
            "irfan/app",
            &draft,
            1 << 20,
            None
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn an_edited_note_and_a_yank_sync_to_the_existing_release() {
    let fake = github_stand_in(true);
    let mut rel = release(ReleaseState::Published, "stable", "first words\n");
    let first = sync(
        &fake,
        Kind::Github,
        "https://github.com",
        "irfan/app",
        &rel,
        1 << 20,
        None,
    )
    .unwrap();
    let patch = body_of(
        &fake,
        "PATCH",
        "https://api.github.com/repos/irfan/app/releases/7",
    );
    assert!(patch["body"].as_str().unwrap().contains("first words"));
    assert!(
        !fake
            .seen()
            .iter()
            .any(|(m, u, _, _)| m == "POST" && u.ends_with("/releases")),
        "the release exists: it is updated"
    );
    assert_eq!(
        fake.seen()
            .iter()
            .filter(|(m, u, _, _)| m == "POST" && u.ends_with("/check-runs"))
            .count(),
        1
    );

    // The owner edits the notes and yanks the release: the release is updated again, and the verdict, which was
    // posted already, is not posted a second time.
    rel.notes = "edited words\n".into();
    rel.state = ReleaseState::Yanked;
    rel.sync = first;
    let rows = sync(
        &fake,
        Kind::Github,
        "https://github.com",
        "irfan/app",
        &rel,
        1 << 20,
        None,
    )
    .unwrap();
    assert_eq!(rows[0].state, "yanked");
    let patch = body_of(
        &fake,
        "PATCH",
        "https://api.github.com/repos/irfan/app/releases/7",
    );
    let body = patch["body"].as_str().unwrap();
    assert!(
        body.contains("edited words") && body.contains("Withdrawn"),
        "{body}"
    );
    assert_eq!(
        fake.seen()
            .iter()
            .filter(|(m, u, _, _)| m == "POST" && u.ends_with("/check-runs"))
            .count(),
        1
    );
}

#[test]
fn an_artifact_too_large_to_attach_is_linked_from_the_notes() {
    let fake = github_stand_in(false);
    let rel = release(ReleaseState::Published, "stable", "notes\n");
    sync(
        &fake,
        Kind::Github,
        "https://github.com",
        "irfan/app",
        &rel,
        4,
        Some("https://hub.example/artifacts"),
    )
    .unwrap();
    let created = body_of(
        &fake,
        "POST",
        "https://api.github.com/repos/irfan/app/releases",
    );
    let body = created["body"].as_str().unwrap();
    assert!(
        body.contains("## Assets") && body.contains("https://hub.example/artifacts/a-1/content"),
        "{body}"
    );
    assert!(
        !fake
            .urls()
            .iter()
            .any(|u| u.ends_with("?name=cargo-app-linux-x86_64")),
        "the artifact was not uploaded"
    );
    assert!(
        fake.urls()
            .iter()
            .any(|u| u.ends_with("?name=cargo-app-linux-x86_64.cdx.json")),
        "its SBOM was"
    );
}

#[test]
fn gitlab_gets_its_release_its_linked_files_and_its_commit_status() {
    let fake = Arc::new(Fake::new(|method, url, _| match (method, url) {
        ("GET", u) if u.contains("/releases/v0.2.0") => {
            (404, json!({ "message": "404 Not found" }))
        }
        ("POST", u) if u.ends_with("/projects/irfan%2Fapp/releases") => (
            201,
            json!({ "tag_name": "v0.2.0", "_links": { "self": "https://gitlab.example/irfan/app/-/releases/v0.2.0" } }),
        ),
        ("PUT", u) if u.contains("/packages/generic/agentplaza/v0.2.0/") => {
            (201, json!({ "message": "201 Created" }))
        }
        ("POST", u) if u.ends_with("/releases/v0.2.0/assets/links") => (201, json!({ "id": 1 })),
        ("POST", u) if u.ends_with("/projects/irfan%2Fapp/statuses/c0ffee") => {
            (201, json!({ "id": 2 }))
        }
        _ => (404, json!({ "message": "no such endpoint" })),
    }));
    let rel = release(ReleaseState::Published, "stable", "notes\n");
    let rows = sync(
        &fake,
        Kind::Gitlab,
        "https://gitlab.example",
        "irfan/app",
        &rel,
        1 << 20,
        None,
    )
    .unwrap();
    assert_eq!(rows[0].platform, key_of(Kind::Gitlab));
    assert_eq!(
        rows[0].url.as_deref(),
        Some("https://gitlab.example/irfan/app/-/releases/v0.2.0")
    );
    let urls = fake.urls();
    assert!(
        urls.iter()
            .any(|u| u == "POST https://gitlab.example/api/v4/projects/irfan%2Fapp/releases"),
        "{urls:?}"
    );
    assert!(urls.iter().any(|u| u.starts_with("PUT https://gitlab.example/api/v4/projects/irfan%2Fapp/packages/generic/agentplaza/v0.2.0/cargo-app-linux-x86_64")));
    assert!(urls.iter().any(|u| u.starts_with(
        "POST https://gitlab.example/api/v4/projects/irfan%2Fapp/releases/v0.2.0/assets/links"
    )));
    let status = body_of(
        &fake,
        "POST",
        "https://gitlab.example/api/v4/projects/irfan%2Fapp/statuses/c0ffee",
    );
    assert_eq!(status["state"], "success");
    assert_eq!(status["name"], "agentplaza/ci");
    assert_eq!(rows[1].platform, "gitlab:check");
}

#[test]
fn forgejo_gets_its_pre_release_its_assets_and_its_status() {
    let fake = Arc::new(Fake::new(|method, url, _| match (method, url) {
        ("GET", u) if u.contains("/releases/tags/") => (404, json!({ "message": "not found" })),
        ("POST", "https://codeberg.org/api/v1/repos/irfan/app/releases") => (
            201,
            json!({ "id": 3, "html_url": "https://codeberg.org/irfan/app/releases/tag/v0.2.0" }),
        ),
        ("POST", u) if u.contains("/releases/3/assets?name=") => (
            201,
            json!({ "browser_download_url": "https://codeberg.org/irfan/app/releases/download/v0.2.0/x" }),
        ),
        ("POST", "https://codeberg.org/api/v1/repos/irfan/app/statuses/c0ffee") => {
            (201, json!({ "id": 4 }))
        }
        _ => (404, json!({ "message": "no such endpoint" })),
    }));
    let rel = release(ReleaseState::Candidate, "beta", "notes\n");
    let rows = sync(
        &fake,
        Kind::Forgejo,
        "https://codeberg.org",
        "irfan/app",
        &rel,
        1 << 20,
        None,
    )
    .unwrap();
    assert_eq!(rows[0].platform, "forgejo");
    assert_eq!(rows[0].state, "pre-release");
    let created = body_of(
        &fake,
        "POST",
        "https://codeberg.org/api/v1/repos/irfan/app/releases",
    );
    assert_eq!(created["prerelease"], true);
    let urls = fake.urls();
    assert!(urls.iter().any(|u| u == "POST https://codeberg.org/api/v1/repos/irfan/app/releases/3/assets?name=cargo-app-linux-x86_64"), "{urls:?}");
    let status = body_of(
        &fake,
        "POST",
        "https://codeberg.org/api/v1/repos/irfan/app/statuses/c0ffee",
    );
    assert_eq!(status["state"], "success");
    assert_eq!(status["context"], "agentplaza/ci");
}
