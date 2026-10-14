//! `agentplaza git`: Git: repositories, branches, changes.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};
use crate::rich;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Whether the git host is set up, and its organisation.
    Status,
    /// The repositories, with their open pull requests.
    List,
    /// Create a repository for the agents to clone and push to.
    Create {
        /// The repository's name.
        name: String,
        /// What it is for; `-` reads stdin.
        #[arg(long)]
        description: Option<String>,
    },
    /// One repository: its facts and branches.
    Show {
        /// The repository's name.
        repo: String,
    },
    /// The branches of a repository, with each one's latest commit.
    Branches {
        /// The repository's name.
        repo: String,
    },
    /// The commits of a branch, newest first.
    Commits {
        /// The repository's name.
        repo: String,
        /// A branch, tag or commit (default: the default branch).
        #[arg(long = "ref", default_value = "")]
        reference: String,
        /// How many (default 30).
        #[arg(long, default_value_t = 30)]
        limit: u32,
    },
    /// The files and folders at a path.
    Tree {
        /// The repository's name.
        repo: String,
        /// A branch, tag or commit (default: the default branch).
        #[arg(long = "ref", default_value = "")]
        reference: String,
        /// The folder (empty for the root).
        #[arg(long, default_value = "")]
        path: String,
    },
    /// The text of one file.
    File {
        /// The repository's name.
        repo: String,
        /// The file's path in the repository.
        path: String,
        /// A branch, tag or commit (default: the default branch).
        #[arg(long = "ref", default_value = "")]
        reference: String,
    },
    /// The pull requests: open (default), closed or all, a page at a time.
    Pulls {
        /// The repository's name.
        repo: String,
        /// open, closed or all.
        #[arg(long, default_value = "open", value_parser = clap::builder::PossibleValuesParser::new(["open", "closed", "all"]))]
        state: String,
        /// The page, from 1.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
        page: u64,
        /// How many per page (1 to 50).
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u64).range(1..=50))]
        limit: u64,
    },
    /// Open a pull request from a branch into another.
    OpenPr {
        /// The repository's name.
        repo: String,
        /// The pull request's title.
        #[arg(long)]
        title: String,
        /// The branch the changes are on.
        #[arg(long)]
        head: String,
        /// The branch they merge into (default: the repository's default branch).
        #[arg(long)]
        base: Option<String>,
        /// The description; `-` reads stdin.
        #[arg(long, conflicts_with = "file")]
        body: Option<String>,
        /// Read the description from a file.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// One pull request: its description, files changed, reviews and discussion.
    Pull {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
    },
    /// The diff of one pull request.
    Diff {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
    },
    /// Review a pull request: approve, request changes, or comment.
    Review {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
        /// approve, request-changes or comment.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["approve", "request-changes", "comment"]))]
        event: String,
        /// The review's text (required unless approving); `-` reads stdin.
        #[arg(long)]
        body: Option<String>,
    },
    /// Comment on a pull request (its discussion).
    Comment {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
        /// The comment's text; `-` reads stdin.
        #[arg(long, conflicts_with = "file")]
        body: Option<String>,
        /// Read the comment's text from a file.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Close a pull request without merging it.
    Close {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
    },
    /// Reopen a closed pull request.
    Reopen {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
    },
    /// Merge a pull request. When it is not approved or not mergeable, it warns and needs --yes.
    Merge {
        /// The repository's name.
        repo: String,
        /// The pull request's number.
        number: u64,
        /// merge, squash or rebase.
        #[arg(long, default_value = "merge", value_parser = clap::builder::PossibleValuesParser::new(["merge", "squash", "rebase"]))]
        style: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// The objects of a JSON array.
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The objects of a JSON array under `key`.
fn array_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).map(array).unwrap_or_default()
}

/// The path of a repository's resource (`rest` is appended, e.g. "/branches").
fn repo_path(repo: &str, rest: &str) -> String {
    format!("/v1/git/repos/{}{rest}", ctx::seg(repo))
}

/// The path of one pull request's resource (`rest` is appended, e.g. "/merge").
fn pull_path(repo: &str, number: u64, rest: &str) -> String {
    repo_path(repo, &format!("/pulls/{number}{rest}"))
}

/// A commit id, shortened for a table.
fn short(id: &str) -> String {
    id.chars().take(10).collect()
}

/// The pull requests' path: the state, the page (from 1) and the limit (1 to 50).
fn pulls_path(repo: &str, state: &str, page: u64, limit: u64) -> String {
    format!(
        "{}?state={}&page={page}&limit={limit}",
        repo_path(repo, "/pulls"),
        ctx::seg(state)
    )
}

/// The URL an agent clones a repository from: the web and the desktop show the same one.
fn clone_url(org: &str, repo: &str) -> String {
    format!("http://plaza.gateway/git/{org}/{repo}.git")
}

/// The text of a `--body` (`-` reads stdin) or a `--file`; None when neither is given or it is blank.
fn text_from(body: Option<String>, file: Option<PathBuf>) -> Result<Option<String>, String> {
    if let Some(file) = file {
        let text =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let text = text.trim();
        return if text.is_empty() {
            Err("the text is empty".into())
        } else {
            Ok(Some(text.to_string()))
        };
    }
    match body.filter(|b| !b.trim().is_empty()) {
        Some(body) => ctx::text_arg(&body).map(Some),
        None => Ok(None),
    }
}

/// Every line of `text` after `pad`.
fn indent(text: &str, pad: &str) -> String {
    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One comment as the screen shows it: its author and time, then its body indented.
fn comment_text(c: &Value) -> String {
    format!(
        "  {}  {}\n{}",
        ctx::s(c, "author"),
        ctx::when(&c["at"]),
        indent(ctx::s(c, "body"), "    ")
    )
}

/// What stands between a pull request and a merge: not approved (or changes requested), or not mergeable.
fn merge_problems(review: &str, mergeable: bool) -> Vec<&'static str> {
    let mut problems = Vec::new();
    match review {
        "approved" => {}
        "changes_requested" => problems.push("has changes requested"),
        _ => problems.push("is not approved"),
    }
    if !mergeable {
        problems.push("is not mergeable");
    }
    problems
}

/// True when the hub says the pull request merged. A hub from before `merged` existed sends no such field: not merged.
fn is_merged(p: &Value) -> bool {
    p["merged"].as_bool() == Some(true)
}

/// A pull request's state as the CLI shows it: `merged` for a merged one (Forgejo keeps it closed), else its state.
fn pull_state(p: &Value) -> &str {
    if is_merged(p) {
        "merged"
    } else {
        ctx::s(p, "state")
    }
}

/// Why a pull request cannot be reopened: a merged one cannot, since its merge closed it for good.
fn reopen_refusal(p: &Value, repo: &str, number: u64) -> Option<String> {
    is_merged(p)
        .then(|| format!("#{number} in {repo} is merged: a merged pull request cannot be reopened"))
}

/// Why a pull request cannot be merged again: a merged one is already in the base branch, and Forgejo would only refuse.
fn merge_refusal(p: &Value, repo: &str, number: u64) -> Option<String> {
    is_merged(p).then(|| format!("#{number} in {repo} is already merged"))
}

/// Closes (`closed`) or reopens (`open`) a pull request and prints it.
fn change_state(ctx: &Ctx, repo: &str, number: u64, state: &str) -> Result<(), String> {
    let p = ctx.client.post(
        &pull_path(repo, number, "/state"),
        &json!({ "state": state }),
    )?;
    let verb = if state == "closed" {
        "closed"
    } else {
        "reopened"
    };
    ctx.out(&p, |_| println!("{verb} #{number} in {repo}"));
    Ok(())
}

/// The branches as a table: name, latest commit, its author and message.
fn print_branches(branches: &Value) {
    let rows: Vec<Vec<String>> = array(branches)
        .iter()
        .map(|b| {
            let commit = b.get("commit").cloned().unwrap_or(Value::Null);
            vec![
                ctx::s(b, "name").to_string(),
                short(ctx::s(&commit, "id")),
                ctx::s(&commit, "author").to_string(),
                ctx::s(&commit, "message").to_string(),
            ]
        })
        .collect();
    ctx::table(&["branch", "commit", "author", "message"], &rows);
}

/// One repository from the list, by name.
fn repo_info(ctx: &Ctx, repo: &str) -> Result<Value, String> {
    let repos = ctx.client.get("/v1/git/repos")?;
    array(&repos)
        .iter()
        .find(|r| ctx::s(r, "name") == repo)
        .cloned()
        .ok_or_else(|| format!("no repository named {repo}"))
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Status => {
            let git = ctx.client.get("/v1/git")?;
            ctx.out(&git, |g| {
                if g.get("configured")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    println!("git host: organisation {}", ctx::s(g, "org"));
                } else {
                    println!("git host: not configured");
                }
            });
            Ok(())
        }
        Cmd::List => {
            let repos = ctx.client.get("/v1/git/repos")?;
            ctx.out(&repos, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|r| {
                        vec![
                            ctx::s(r, "name").to_string(),
                            ctx::s(r, "default_branch").to_string(),
                            ctx::text(r, "open_prs"),
                            ctx::text(r, "project"),
                            ctx::s(r, "updated_at").to_string(),
                            ctx::s(r, "description").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(
                    &[
                        "name",
                        "branch",
                        "open prs",
                        "project",
                        "updated",
                        "description",
                    ],
                    &rows,
                );
            });
            Ok(())
        }
        Cmd::Create { name, description } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err("name the repository".into());
            }
            let description = description
                .filter(|d| !d.trim().is_empty())
                .map(|d| ctx::text_arg(&d))
                .transpose()?
                .unwrap_or_default();
            let repo = ctx.client.post(
                "/v1/git/repos",
                &json!({ "name": name, "description": description }),
            )?;
            ctx.out(&repo, |r| {
                println!("created repository {}", ctx::s(r, "name"))
            });
            Ok(())
        }
        Cmd::Show { repo } => {
            let info = repo_info(ctx, &repo)?;
            let branches = ctx.client.get(&repo_path(&repo, "/branches"))?;
            let host = ctx.client.get("/v1/git")?;
            let out = json!({ "repo": info, "branches": branches });
            ctx.out(&out, |_| {
                ctx::fields(&[
                    ("name", ctx::s(&info, "name").to_string()),
                    ("description", ctx::s(&info, "description").to_string()),
                    (
                        "default branch",
                        ctx::s(&info, "default_branch").to_string(),
                    ),
                    (
                        "clone",
                        clone_url(ctx::s(&host, "org"), ctx::s(&info, "name")),
                    ),
                    ("open pull requests", ctx::text(&info, "open_prs")),
                    ("project", ctx::text(&info, "project")),
                    ("updated", ctx::s(&info, "updated_at").to_string()),
                ]);
                println!("\nBranches");
                print_branches(&branches);
            });
            Ok(())
        }
        Cmd::Branches { repo } => {
            let branches = ctx.client.get(&repo_path(&repo, "/branches"))?;
            ctx.out(&branches, print_branches);
            Ok(())
        }
        Cmd::Commits {
            repo,
            reference,
            limit,
        } => {
            let path = format!(
                "{}?ref={}&limit={limit}",
                repo_path(&repo, "/commits"),
                ctx::seg(&reference)
            );
            let commits = ctx.client.get(&path)?;
            ctx.out(&commits, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|c| {
                        vec![
                            ctx::s(c, "short").to_string(),
                            ctx::s(c, "author").to_string(),
                            ctx::when(&c["at"]),
                            format!(
                                "+{} -{}",
                                ctx::text(c, "additions"),
                                ctx::text(c, "deletions")
                            ),
                            ctx::s(c, "message")
                                .lines()
                                .next()
                                .unwrap_or("")
                                .to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["commit", "author", "at", "changes", "message"], &rows);
            });
            Ok(())
        }
        Cmd::Tree {
            repo,
            reference,
            path,
        } => {
            let url = format!(
                "{}?ref={}&path={}",
                repo_path(&repo, "/tree"),
                ctx::seg(&reference),
                ctx::seg(&path)
            );
            let entries = ctx.client.get(&url)?;
            ctx.out(&entries, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|e| {
                        vec![
                            ctx::s(e, "kind").to_string(),
                            ctx::text(e, "size"),
                            ctx::s(e, "path").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["kind", "size", "path"], &rows);
            });
            Ok(())
        }
        Cmd::File {
            repo,
            path,
            reference,
        } => {
            let url = format!(
                "{}?ref={}&path={}",
                repo_path(&repo, "/file"),
                ctx::seg(&reference),
                ctx::seg(&path)
            );
            let file = ctx.client.get(&url)?;
            ctx.out(&file, |f| {
                if f.get("binary").and_then(Value::as_bool).unwrap_or(false) {
                    println!(
                        "{} is a binary file ({} bytes)",
                        ctx::s(f, "path"),
                        ctx::text(f, "size")
                    );
                } else {
                    println!("{}", ctx::s(f, "text"));
                }
            });
            Ok(())
        }
        Cmd::Pulls {
            repo,
            state,
            page,
            limit,
        } => {
            let pulls = ctx.client.get(&pulls_path(&repo, &state, page, limit))?;
            ctx.out(&pulls, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::text(p, "number"),
                            pull_state(p).to_string(),
                            ctx::s(p, "review").to_string(),
                            ctx::s(p, "author").to_string(),
                            format!("{} -> {}", ctx::s(p, "head"), ctx::s(p, "base")),
                            ctx::s(p, "title").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(
                    &["number", "state", "review", "author", "flow", "title"],
                    &rows,
                );
            });
            Ok(())
        }
        Cmd::Pull { repo, number } => {
            let p = ctx.client.get(&pull_path(&repo, number, ""))?;
            let rich_ctx = rich::context_for(ctx);
            ctx.out(&p, |p| {
                ctx::fields(&[
                    ("number", format!("#{}", ctx::text(p, "number"))),
                    ("title", ctx::text(p, "title")),
                    ("state", pull_state(p).to_string()),
                    ("review", ctx::text(p, "review")),
                    ("author", ctx::text(p, "author")),
                    (
                        "flow",
                        format!("{} -> {}", ctx::s(p, "head"), ctx::s(p, "base")),
                    ),
                    ("mergeable", ctx::text(p, "mergeable")),
                    ("comments", ctx::text(p, "comments")),
                    ("updated", ctx::text(p, "updated_at")),
                ]);
                let body = ctx::s(p, "body");
                let body = rich::render(body, &rich_ctx, &ctx.rich);
                println!(
                    "\n{}",
                    if body.is_empty() {
                        "No description provided.".to_string()
                    } else {
                        body
                    }
                );
                println!("\nFiles changed ({})", array_at(p, "files").len());
                for f in array_at(p, "files") {
                    println!(
                        "  +{} -{}  {}",
                        ctx::text(f, "additions"),
                        ctx::text(f, "deletions"),
                        ctx::s(f, "path")
                    );
                }
                println!("\nReviews ({})", array_at(p, "reviews").len());
                for r in array_at(p, "reviews") {
                    let text = rich::render(ctx::s(r, "body"), &rich_ctx, &ctx.rich);
                    println!("  {} {}: {text}", ctx::s(r, "author"), ctx::s(r, "state"));
                }
                let discussion = array_at(p, "discussion");
                println!("\nDiscussion ({})", discussion.len());
                if discussion.is_empty() {
                    println!("  No comments yet.");
                }
                for c in discussion {
                    println!("\n{}", rich::render(&comment_text(c), &rich_ctx, &ctx.rich));
                }
            });
            Ok(())
        }
        Cmd::Diff { repo, number } => {
            let p = ctx.client.get(&pull_path(&repo, number, ""))?;
            ctx.out(&p, |p| println!("{}", ctx::s(p, "diff")));
            Ok(())
        }
        Cmd::Review {
            repo,
            number,
            event,
            body,
        } => {
            let event = match event.as_str() {
                "approve" => "APPROVE",
                "request-changes" => "REQUEST_CHANGES",
                _ => "COMMENT",
            };
            let body = body
                .filter(|b| !b.trim().is_empty())
                .map(|b| ctx::text_arg(&b))
                .transpose()?
                .unwrap_or_default();
            if event != "APPROVE" && body.is_empty() {
                return Err("write the review's text first (--body)".into());
            }
            let r = ctx.client.post(
                &pull_path(&repo, number, "/review"),
                &json!({ "event": event, "body": body }),
            )?;
            ctx.out(&r, |_| println!("reviewed #{number} in {repo}: {event}"));
            Ok(())
        }
        Cmd::OpenPr {
            repo,
            title,
            head,
            base,
            body,
            file,
        } => {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err("title the pull request".into());
            }
            let head = head.trim().to_string();
            if head.is_empty() {
                return Err("name the branch the changes are on (--head)".into());
            }
            let base = match base.map(|b| b.trim().to_string()).filter(|b| !b.is_empty()) {
                Some(base) => base,
                None => ctx::s(&repo_info(ctx, &repo)?, "default_branch").to_string(),
            };
            if base.is_empty() {
                return Err("the repository has no default branch: name one with --base".into());
            }
            let body = text_from(body, file)?.unwrap_or_default();
            let input = json!({ "title": title, "head": head, "base": base, "body": body });
            let p = ctx.client.post(&repo_path(&repo, "/pulls"), &input)?;
            ctx.out(&p, |p| {
                println!(
                    "opened #{} in {repo}: {head} -> {base}",
                    ctx::text(p, "number")
                )
            });
            Ok(())
        }
        Cmd::Comment {
            repo,
            number,
            body,
            file,
        } => {
            let text = text_from(body, file)?
                .ok_or("write the comment's text first (--body or --file)")?;
            let c = ctx.client.post(
                &pull_path(&repo, number, "/comments"),
                &json!({ "body": text }),
            )?;
            ctx.out(&c, |c| {
                println!("commented on #{number} in {repo}\n{}", comment_text(c))
            });
            Ok(())
        }
        Cmd::Close { repo, number } => change_state(ctx, &repo, number, "closed"),
        Cmd::Reopen { repo, number } => {
            let p = ctx.client.get(&pull_path(&repo, number, ""))?;
            if let Some(refusal) = reopen_refusal(&p, &repo, number) {
                return Err(refusal);
            }
            change_state(ctx, &repo, number, "open")
        }
        Cmd::Merge {
            repo,
            number,
            style,
            yes,
        } => {
            let p = ctx.client.get(&pull_path(&repo, number, ""))?;
            if let Some(refusal) = merge_refusal(&p, &repo, number) {
                return Err(refusal);
            }
            let problems = merge_problems(
                ctx::s(&p, "review"),
                p["mergeable"].as_bool().unwrap_or(false),
            );
            if problems.is_empty() {
                if !confirm(yes, &format!("Merge #{number} in {repo} ({style})."))? {
                    return Err("not done".into());
                }
            } else {
                for problem in &problems {
                    eprintln!("warning: #{number} {problem}");
                }
                if !yes {
                    return Err(format!(
                        "#{number} is not ready to merge: pass --yes to merge it anyway"
                    ));
                }
            }
            let r = ctx.client.post(
                &pull_path(&repo, number, "/merge"),
                &json!({ "style": style }),
            )?;
            ctx.out(&r, |_| println!("merged #{number} in {repo} ({style})"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulls_path_carries_the_state_page_and_limit() {
        assert_eq!(
            pulls_path("demo", "closed", 2, 10),
            "/v1/git/repos/demo/pulls?state=closed&page=2&limit=10"
        );
    }

    #[test]
    fn clone_url_is_the_agents_gateway_address() {
        assert_eq!(
            clone_url("agentplaza", "demo"),
            "http://plaza.gateway/git/agentplaza/demo.git"
        );
    }

    #[test]
    fn text_from_takes_the_body_or_nothing() {
        assert_eq!(
            text_from(Some("  hi  ".into()), None).unwrap(),
            Some("hi".into())
        );
        assert_eq!(text_from(Some("   ".into()), None).unwrap(), None);
        assert_eq!(text_from(None, None).unwrap(), None);
    }

    #[test]
    fn comment_text_indents_the_body_under_author_and_time() {
        let c = json!({"author": "alice", "body": "one\ntwo", "at": "2026-10-14 10:00"});
        assert_eq!(
            comment_text(&c),
            "  alice  2026-10-14 10:00\n    one\n    two"
        );
    }

    #[test]
    fn the_pull_request_commands_parse_their_flags() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrap {
            #[command(subcommand)]
            cmd: Cmd,
        }
        let parse = |args: &[&str]| {
            Wrap::try_parse_from(std::iter::once("git").chain(args.iter().copied())).map(|w| w.cmd)
        };
        assert!(parse(&["pulls", "demo", "--page", "2", "--limit", "50"]).is_ok());
        assert!(
            parse(&["pulls", "demo", "--limit", "51"]).is_err(),
            "a page holds at most 50"
        );
        assert!(
            parse(&["pulls", "demo", "--page", "0"]).is_err(),
            "pages start at 1"
        );
        assert!(parse(&["open-pr", "demo", "--title", "t", "--head", "feature"]).is_ok());
        assert!(parse(&["comment", "demo", "3", "--body", "-"]).is_ok());
        assert!(
            parse(&["comment", "demo", "3", "--body", "a", "--file", "f.md"]).is_err(),
            "one source of text"
        );
        assert!(parse(&["close", "demo", "3"]).is_ok());
        assert!(parse(&["reopen", "demo", "3"]).is_ok());
    }

    #[test]
    fn a_merged_pull_request_shows_as_merged_and_cannot_be_reopened() {
        // Forgejo keeps a merged pull request closed; the hub says it merged.
        let merged = json!({"number": 4, "state": "closed", "merged": true, "merged_at": "2026-10-03T09:30:00Z"});
        assert_eq!(pull_state(&merged), "merged");
        assert!(is_merged(&merged));
        let refusal = reopen_refusal(&merged, "demo", 4).expect("a merged pull request is refused");
        assert!(
            refusal.contains("#4")
                && refusal.contains("merged")
                && refusal.contains("cannot be reopened"),
            "{refusal}"
        );

        // A closed pull request that did not merge can still be reopened.
        let closed = json!({"number": 5, "state": "closed", "merged": false, "merged_at": null});
        assert_eq!(pull_state(&closed), "closed");
        assert!(reopen_refusal(&closed, "demo", 5).is_none());

        // An older hub sends no `merged`: the state stands, and a closed one reopens.
        let older = json!({"number": 6, "state": "closed"});
        assert_eq!(pull_state(&older), "closed");
        assert!(reopen_refusal(&older, "demo", 6).is_none());
        assert_eq!(pull_state(&json!({"number": 7, "state": "open"})), "open");
    }

    #[test]
    fn merge_problems_are_empty_only_for_an_approved_mergeable_pull_request() {
        assert!(merge_problems("approved", true).is_empty());
        assert_eq!(merge_problems("pending", true), vec!["is not approved"]);
        assert_eq!(
            merge_problems("changes_requested", true),
            vec!["has changes requested"]
        );
        assert_eq!(merge_problems("approved", false), vec!["is not mergeable"]);
        assert_eq!(
            merge_problems("pending", false),
            vec!["is not approved", "is not mergeable"]
        );
    }

    #[test]
    fn a_merged_pull_request_is_refused_before_a_merge_is_sent() {
        let merged = json!({"number": 4, "state": "closed", "merged": true, "merged_at": "2026-10-03T09:30:00Z"});
        let refusal = merge_refusal(&merged, "demo", 4).expect("a merged pull request is refused");
        assert_eq!(refusal, "#4 in demo is already merged");
        // An open pull request, a closed one that did not merge, and an older hub's (no `merged`) can be merged.
        assert!(
            merge_refusal(
                &json!({"number": 5, "state": "open", "merged": false}),
                "demo",
                5
            )
            .is_none()
        );
        assert!(
            merge_refusal(
                &json!({"number": 6, "state": "closed", "merged": false}),
                "demo",
                6
            )
            .is_none()
        );
        assert!(merge_refusal(&json!({"number": 7, "state": "open"}), "demo", 7).is_none());
    }
}
