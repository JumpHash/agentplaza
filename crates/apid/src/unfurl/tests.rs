//! Unfurl tests: the cards built from recorded GitHub and Forgejo answers (fixtures under `fixtures/`), the cache's
//! time to live with an injected clock, the checks on what a client may ask for, and the error for each failed answer.
//! No network: `Fixtures` answers every provider call.

use std::collections::HashMap;
use std::sync::Mutex;

use apz_rich::Ref;
use serde_json::{Value, json};

use super::*;

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("the fixture parses")
}

/// A provider that answers from a table (anything else is a 404) and records every call it is asked.
#[derive(Default)]
struct Fixtures {
    github: HashMap<String, (u16, Value)>,
    forge: HashMap<String, (u16, Value)>,
    origin: String,
    forge_error: Option<&'static str>,
    calls: Mutex<Vec<String>>,
}

impl Fixtures {
    fn answer_github(mut self, path: &str, status: u16, body: Value) -> Self {
        self.github.insert(path.to_string(), (status, body));
        self
    }

    fn answer_forge(mut self, path: &str, status: u16, body: Value) -> Self {
        self.forge.insert(path.to_string(), (status, body));
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Source for Fixtures {
    fn github(&self, path: &str) -> Result<(u16, Value), &'static str> {
        self.calls.lock().unwrap().push(format!("github {path}"));
        Ok(self
            .github
            .get(path)
            .cloned()
            .unwrap_or((404, json!({"message": "Not Found"}))))
    }

    fn forge(&self, path: &str) -> Result<(u16, Value), &'static str> {
        self.calls.lock().unwrap().push(format!("forge {path}"));
        if let Some(error) = self.forge_error {
            return Err(error);
        }
        Ok(self
            .forge
            .get(path)
            .cloned()
            .unwrap_or((404, json!({"message": "not found"}))))
    }

    fn forge_origin(&self) -> String {
        self.origin.clone()
    }
}

const PLAZA: &str = "https://git.plaza.test";

fn github_pull_ref() -> Ref {
    Ref::GithubPull {
        owner: "o".into(),
        repo: "r".into(),
        number: 12,
    }
}

fn plaza_pull_ref() -> Ref {
    Ref::PlazaGitPull {
        owner: "agentplaza".into(),
        repo: "pr-1".into(),
        number: 2,
    }
}

fn card_of(item: &Value) -> &Value {
    assert_eq!(item["ok"], true, "an ok answer: {item}");
    &item["card"]
}

#[test]
fn a_github_pull_card_comes_from_the_recorded_answer() {
    let src = Fixtures::default().answer_github(
        "/repos/o/r/pulls/12",
        200,
        fixture(include_str!("fixtures/github_pull.json")),
    );
    let item = resolve_remote(&src, &Cache::default(), &github_pull_ref(), 0);
    let card = card_of(&item);
    assert_eq!(card["kind"], "github_pull");
    assert_eq!(card["title"], "Add the rich content classifier");
    assert_eq!(card["state"], "open");
    assert_eq!(
        card["author"],
        json!({"login": "octocat", "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"})
    );
    assert_eq!(card["head"], "feature");
    assert_eq!(card["base"], "main");
    assert_eq!(
        (
            card["additions"].as_u64(),
            card["deletions"].as_u64(),
            card["comments"].as_u64()
        ),
        (Some(812), Some(14), Some(3))
    );
    assert_eq!(card["created_ms"], 1_791_624_600_000u64);
    assert_eq!(
        card["body_excerpt"],
        "Summary This adds apz-rich and the spec(https://example.com/spec). Pure, no I/O."
    );
    assert_eq!(card["url"], "https://github.com/o/r/pull/12");
    assert_eq!(src.calls(), ["github /repos/o/r/pulls/12"]);
}

#[test]
fn a_plaza_pull_that_merged_is_merged_and_links_to_the_plaza() {
    let src = Fixtures {
        origin: PLAZA.into(),
        ..Default::default()
    }
    .answer_forge(
        "/api/v1/repos/agentplaza/pr-1/pulls/2",
        200,
        fixture(include_str!("fixtures/forgejo_pull.json")),
    );
    let item = resolve_remote(&src, &Cache::default(), &plaza_pull_ref(), 0);
    let card = card_of(&item);
    assert_eq!(card["kind"], "plaza_git_pull");
    assert_eq!(card["state"], "merged", "merged wins over closed");
    assert_eq!(card["author"]["login"], "zarathos");
    assert_eq!(
        card["created_ms"], 1_791_525_600_000u64,
        "an offset (+02:00) is applied"
    );
    assert_eq!(
        card["url"],
        "https://git.plaza.test/agentplaza/pr-1/pulls/2"
    );
    assert_eq!(src.calls(), ["forge /api/v1/repos/agentplaza/pr-1/pulls/2"]);
}

#[test]
fn an_issue_card_keeps_its_label_names_and_the_body_start() {
    let src = Fixtures::default().answer_github(
        "/repos/o/r/issues/7",
        200,
        fixture(include_str!("fixtures/github_issue.json")),
    );
    let r = Ref::GithubIssue {
        owner: "o".into(),
        repo: "r".into(),
        number: 7,
    };
    let card = card_of(&resolve_remote(&src, &Cache::default(), &r, 0)).clone();
    assert_eq!(card["kind"], "github_issue");
    assert_eq!(card["state"], "open");
    assert_eq!(card["labels"], json!(["bug", "good first issue"]));
    assert_eq!(card["comments"], 2);
    assert_eq!(
        card["body_excerpt"],
        "Steps: 1. run tool with no args 2. see the panic"
    );
    assert_eq!(card["url"], "https://github.com/o/r/issues/7");
}

#[test]
fn a_commit_card_has_the_message_author_date_and_size() {
    let sha = "abc1234def56789abc1234def56789abc1234de";
    let src = Fixtures::default().answer_github(
        &format!("/repos/o/r/commits/{sha}"),
        200,
        fixture(include_str!("fixtures/github_commit.json")),
    );
    let r = Ref::GithubCommit {
        owner: "o".into(),
        repo: "r".into(),
        sha: sha.into(),
    };
    let card = card_of(&resolve_remote(&src, &Cache::default(), &r, 0)).clone();
    assert_eq!(card["kind"], "github_commit");
    assert_eq!(card["title"], "Fix the parser");
    assert_eq!(
        card["message"],
        "Fix the parser The lexer read past the end of the buffer."
    );
    assert_eq!(
        card["author"],
        json!({"name": "Octo Cat", "login": "octocat", "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"})
    );
    assert_eq!(card["date_ms"], 1_791_210_000_000u64);
    assert_eq!(
        (
            card["files"].as_u64(),
            card["additions"].as_u64(),
            card["deletions"].as_u64()
        ),
        (Some(2), Some(21), Some(3))
    );
    assert_eq!(card["url"], format!("https://github.com/o/r/commit/{sha}"));
}

#[test]
fn a_plaza_commit_reads_the_git_commits_route() {
    let sha = "1122334455667788990011223344556677889900";
    let src = Fixtures {
        origin: PLAZA.into(),
        ..Default::default()
    }
    .answer_forge(
        &format!("/api/v1/repos/agentplaza/pr-1/git/commits/{sha}"),
        200,
        fixture(include_str!("fixtures/forgejo_commit.json")),
    );
    let r = Ref::PlazaGitCommit {
        owner: "agentplaza".into(),
        repo: "pr-1".into(),
        sha: sha.into(),
    };
    let card = card_of(&resolve_remote(&src, &Cache::default(), &r, 0)).clone();
    assert_eq!(card["title"], "Tidy the cards");
    assert_eq!(card["message"], "Tidy the cards");
    assert_eq!(card["date_ms"], 1_791_270_000_000u64);
    assert_eq!(card["files"], 1);
}

#[test]
fn a_repo_card_reads_stars_under_either_name() {
    let github = Fixtures::default().answer_github(
        "/repos/o/r",
        200,
        fixture(include_str!("fixtures/github_repo.json")),
    );
    let card = card_of(&resolve_remote(
        &github,
        &Cache::default(),
        &Ref::GithubRepo {
            owner: "o".into(),
            repo: "r".into(),
        },
        0,
    ))
    .clone();
    assert_eq!(
        (
            card["name"].as_str(),
            card["stars"].as_u64(),
            card["language"].as_str()
        ),
        (Some("o/r"), Some(1234), Some("Rust"))
    );
    assert_eq!(card["updated_ms"], 1_791_878_400_000u64);

    let forge = Fixtures {
        origin: PLAZA.into(),
        ..Default::default()
    }
    .answer_forge(
        "/api/v1/repos/agentplaza/pr-1",
        200,
        fixture(include_str!("fixtures/forgejo_repo.json")),
    );
    let r = Ref::PlazaGitRepo {
        owner: "agentplaza".into(),
        repo: "pr-1".into(),
    };
    let card = card_of(&resolve_remote(&forge, &Cache::default(), &r, 0)).clone();
    assert_eq!(
        (card["stars"].as_u64(), card["description"].as_str()),
        (Some(3), Some("Project one"))
    );
    assert_eq!(card["url"], "https://git.plaza.test/agentplaza/pr-1");
}

#[test]
fn a_file_card_carries_its_first_lines_and_the_line_it_links_to() {
    let src = Fixtures::default().answer_github(
        "/repos/o/r/contents/src/main.rs?ref=main",
        200,
        fixture(include_str!("fixtures/github_file.json")),
    );
    let r = Ref::GithubFile {
        owner: "o".into(),
        repo: "r".into(),
        reference: "main".into(),
        path: "src/main.rs".into(),
        line: Some(2),
        dir: false,
    };
    let card = card_of(&resolve_remote(&src, &Cache::default(), &r, 0)).clone();
    assert_eq!(
        card["lines"],
        json!(["fn main() {", "    println!(\"hi\");", "}"])
    );
    assert_eq!(
        (
            card["dir"].as_bool(),
            card["binary"].as_bool(),
            card["truncated"].as_bool()
        ),
        (Some(false), Some(false), Some(false))
    );
    assert_eq!(card["line"], 2);
    assert_eq!(
        card["url"],
        "https://github.com/o/r/blob/main/src/main.rs#L2"
    );
}

#[test]
fn a_binary_file_has_no_lines_and_a_folder_lists_its_names() {
    let binary =
        json!({"type": "file", "size": 7, "content": "YmluAGFyeQ==", "encoding": "base64"});
    let src =
        Fixtures::default().answer_github("/repos/o/r/contents/data.bin?ref=main", 200, binary);
    let r = Ref::GithubFile {
        owner: "o".into(),
        repo: "r".into(),
        reference: "main".into(),
        path: "data.bin".into(),
        line: None,
        dir: false,
    };
    let card = card_of(&resolve_remote(&src, &Cache::default(), &r, 0)).clone();
    assert_eq!(
        (card["binary"].as_bool(), card["lines"].is_null()),
        (Some(true), true)
    );

    let src = Fixtures::default().answer_github(
        "/repos/o/r/contents/crates?ref=main",
        200,
        fixture(include_str!("fixtures/github_dir.json")),
    );
    let r = Ref::GithubFile {
        owner: "o".into(),
        repo: "r".into(),
        reference: "main".into(),
        path: "crates".into(),
        line: None,
        dir: true,
    };
    let card = card_of(&resolve_remote(&src, &Cache::default(), &r, 0)).clone();
    assert_eq!(
        (card["dir"].as_bool(), card["entries"].as_u64()),
        (Some(true), Some(3))
    );
    assert_eq!(card["names"], json!(["lib.rs", "main.rs", "tests"]));
    assert_eq!(card["url"], "https://github.com/o/r/tree/main/crates");
}

#[test]
fn run_release_and_gist_cards() {
    let src = Fixtures::default()
        .answer_github(
            "/repos/o/r/actions/runs/9",
            200,
            fixture(include_str!("fixtures/github_run.json")),
        )
        .answer_github(
            "/repos/o/r/releases/tags/v1.2",
            200,
            fixture(include_str!("fixtures/github_release.json")),
        )
        .answer_github(
            "/gists/abc123",
            200,
            fixture(include_str!("fixtures/github_gist.json")),
        );
    let store = Cache::default();

    let run = card_of(&resolve_remote(
        &src,
        &store,
        &Ref::GithubRun {
            owner: "o".into(),
            repo: "r".into(),
            id: 9,
        },
        0,
    ))
    .clone();
    assert_eq!(
        (
            run["state"].as_str(),
            run["conclusion"].as_str(),
            run["branch"].as_str()
        ),
        (Some("completed"), Some("success"), Some("main"))
    );
    assert_eq!(run["created_ms"], 1_791_885_600_000u64);

    let release = card_of(&resolve_remote(
        &src,
        &store,
        &Ref::GithubRelease {
            owner: "o".into(),
            repo: "r".into(),
            tag: "v1.2".into(),
        },
        0,
    ))
    .clone();
    assert_eq!(
        (release["name"].as_str(), release["prerelease"].as_bool()),
        (Some("v1.2"), Some(true)),
        "an empty name falls back to the tag"
    );
    assert_eq!(
        release["created_ms"], 1_791_072_000_000u64,
        "the publish time, not the creation time"
    );

    let gist = card_of(&resolve_remote(
        &src,
        &store,
        &Ref::Gist {
            id: "abc123".into(),
        },
        0,
    ))
    .clone();
    assert_eq!(
        (gist["title"].as_str(), gist["files"].as_u64()),
        (Some("notes.md"), Some(2))
    );
    assert_eq!(gist["updated_ms"], 1_790_769_600_000u64);
    assert_eq!(gist["url"], "https://gist.github.com/abc123");
}

#[test]
fn a_failed_answer_says_why() {
    let rate_403 = Fixtures::default().answer_github(
        "/repos/o/r/pulls/12",
        403,
        json!({"message": "API rate limit exceeded for 1.2.3.4."}),
    );
    assert_eq!(
        resolve_remote(&rate_403, &Cache::default(), &github_pull_ref(), 0),
        json!({"ok": false, "error": "rate_limited"})
    );

    let rate_429 = Fixtures::default().answer_github("/repos/o/r/pulls/12", 429, json!({}));
    assert_eq!(
        resolve_remote(&rate_429, &Cache::default(), &github_pull_ref(), 0)["error"],
        "rate_limited"
    );

    let forbidden = Fixtures::default().answer_github(
        "/repos/o/r/pulls/12",
        403,
        json!({"message": "Resource not accessible"}),
    );
    assert_eq!(
        resolve_remote(&forbidden, &Cache::default(), &github_pull_ref(), 0)["error"],
        "unavailable"
    );

    let missing = Fixtures::default();
    assert_eq!(
        resolve_remote(&missing, &Cache::default(), &github_pull_ref(), 0)["error"],
        "not_found"
    );

    let down = Fixtures {
        forge_error: Some("not_configured"),
        ..Default::default()
    };
    assert_eq!(
        resolve_remote(&down, &Cache::default(), &plaza_pull_ref(), 0)["error"],
        "not_configured"
    );
}

#[test]
fn only_allow_listed_calls_with_checked_names_reach_a_provider() {
    let src = Fixtures::default();
    let store = Cache::default();
    let refused = [
        Ref::GithubRepo {
            owner: "../admin".into(),
            repo: "r".into(),
        },
        Ref::GithubFile {
            owner: "o".into(),
            repo: "r".into(),
            reference: "main".into(),
            path: "a/../../b".into(),
            line: None,
            dir: false,
        },
        Ref::GithubCommit {
            owner: "o".into(),
            repo: "r".into(),
            sha: "xyz".into(),
        },
        Ref::Gist { id: "a/b".into() },
        Ref::PlazaGitRepo {
            owner: "agentplaza".into(),
            repo: "x?y".into(),
        },
        Ref::PlazaGitRun {
            owner: "agentplaza".into(),
            repo: "pr-1".into(),
            id: 4,
        },
        Ref::Path {
            path: "src/main.rs".into(),
            line: None,
            dir: false,
        },
        Ref::Image {
            url: "https://x.io/a.png".into(),
        },
    ];
    for r in &refused {
        assert_eq!(
            resolve_remote(&src, &store, r, 0)["error"],
            "unsupported",
            "{r:?}"
        );
    }
    assert!(
        src.calls().is_empty(),
        "no provider was asked: {:?}",
        src.calls()
    );
}

#[test]
fn the_plaza_routes_carry_the_api_prefix_and_github_does_not() {
    assert_eq!(
        plan(&plaza_pull_ref()),
        Some(Call {
            host: Host::Forge,
            path: "/api/v1/repos/agentplaza/pr-1/pulls/2".into()
        })
    );
    assert_eq!(
        plan(&github_pull_ref()),
        Some(Call {
            host: Host::GitHub,
            path: "/repos/o/r/pulls/12".into()
        })
    );
    assert_eq!(
        plan(&Ref::PlazaGitFile {
            owner: "agentplaza".into(),
            repo: "pr-1".into(),
            reference: "feature/x".into(),
            path: "a b/c.rs".into(),
            line: None,
            dir: false
        }),
        Some(Call {
            host: Host::Forge,
            path: "/api/v1/repos/agentplaza/pr-1/contents/a%20b/c.rs?ref=feature%2Fx".into()
        })
    );
}

#[test]
fn resolve_caches_each_answer_for_its_ttl() {
    // an open item is served from the cache for ten minutes, then refetched
    {
        let src = Fixtures::default().answer_github(
            "/repos/o/r/pulls/12",
            200,
            fixture(include_str!("fixtures/github_pull.json")),
        );
        let store = Cache::default();
        resolve_remote(&src, &store, &github_pull_ref(), 0);
        resolve_remote(&src, &store, &github_pull_ref(), cards::OPEN_TTL - 1);
        assert_eq!(src.calls().len(), 1, "still fresh: the cache answered");
        resolve_remote(&src, &store, &github_pull_ref(), cards::OPEN_TTL);
        assert_eq!(src.calls().len(), 2, "ten minutes on: fetched again");
    }
    // a merged item is kept an hour, and a failure a minute
    {
        let merged = Fixtures {
            origin: PLAZA.into(),
            ..Default::default()
        }
        .answer_forge(
            "/api/v1/repos/agentplaza/pr-1/pulls/2",
            200,
            fixture(include_str!("fixtures/forgejo_pull.json")),
        );
        let store = Cache::default();
        resolve_remote(&merged, &store, &plaza_pull_ref(), 0);
        resolve_remote(&merged, &store, &plaza_pull_ref(), cards::SETTLED_TTL - 1);
        assert_eq!(merged.calls().len(), 1);
        resolve_remote(&merged, &store, &plaza_pull_ref(), cards::SETTLED_TTL);
        assert_eq!(merged.calls().len(), 2);

        let missing = Fixtures::default();
        let store = Cache::default();
        resolve_remote(&missing, &store, &github_pull_ref(), 0);
        resolve_remote(&missing, &store, &github_pull_ref(), cards::FAILURE_TTL - 1);
        assert_eq!(missing.calls().len(), 1, "a failure is served for a minute");
        resolve_remote(&missing, &store, &github_pull_ref(), cards::FAILURE_TTL);
        assert_eq!(missing.calls().len(), 2);
    }
}

#[test]
fn the_time_to_live_follows_the_state_and_the_kind() {
    let open = json!({"ok": true, "card": {"kind": "github_pull", "state": "open"}});
    let draft = json!({"ok": true, "card": {"kind": "github_pull", "state": "draft"}});
    let merged = json!({"ok": true, "card": {"kind": "github_pull", "state": "merged"}});
    let closed = json!({"ok": true, "card": {"kind": "github_issue", "state": "closed"}});
    let run_done = json!({"ok": true, "card": {"kind": "github_run", "state": "completed"}});
    let commit = json!({"ok": true, "card": {"kind": "plaza_git_commit", "state": null}});
    let repo = json!({"ok": true, "card": {"kind": "github_repo"}});
    let failure = json!({"ok": false, "error": "not_found"});
    assert_eq!(cards::ttl_ms(&open), cards::OPEN_TTL);
    assert_eq!(cards::ttl_ms(&draft), cards::OPEN_TTL);
    assert_eq!(cards::ttl_ms(&merged), cards::SETTLED_TTL);
    assert_eq!(cards::ttl_ms(&closed), cards::SETTLED_TTL);
    assert_eq!(cards::ttl_ms(&run_done), cards::SETTLED_TTL);
    assert_eq!(cards::ttl_ms(&commit), cards::SETTLED_TTL);
    assert_eq!(cards::ttl_ms(&repo), cards::OPEN_TTL);
    assert_eq!(cards::ttl_ms(&failure), cards::FAILURE_TTL);
}

#[test]
fn the_memory_copy_stays_within_its_limit() {
    let cache = Cache::default();
    for i in 0..(MEMORY_LIMIT + 10) {
        cache.put(
            &format!("k{i}"),
            Row {
                data: json!({}),
                ok: true,
                fetched_ms: i as u64,
            },
        );
    }
    let rows = cache.rows.lock().unwrap();
    assert_eq!(rows.len(), MEMORY_LIMIT);
    assert!(!rows.contains_key("k0"), "the stalest answer went first");
}

#[test]
fn the_web_links_point_at_the_pages() {
    let origin = PLAZA;
    assert_eq!(
        cards::web_url(
            &Ref::GithubIssue {
                owner: "o".into(),
                repo: "r".into(),
                number: 7
            },
            ""
        ),
        Some("https://github.com/o/r/issues/7".into())
    );
    assert_eq!(
        cards::web_url(
            &Ref::PlazaGitFile {
                owner: "agentplaza".into(),
                repo: "pr-1".into(),
                reference: "main".into(),
                path: "a b.rs".into(),
                line: Some(4),
                dir: false
            },
            origin
        ),
        Some("https://git.plaza.test/agentplaza/pr-1/src/branch/main/a%20b.rs#L4".into())
    );
    assert_eq!(
        cards::web_url(
            &Ref::PlazaGitRepo {
                owner: "agentplaza".into(),
                repo: "pr-1".into()
            },
            ""
        ),
        None,
        "no origin, no link"
    );
    assert_eq!(
        cards::web_url(
            &Ref::Gist {
                id: "abc123".into()
            },
            ""
        ),
        Some("https://gist.github.com/abc123".into())
    );
}
