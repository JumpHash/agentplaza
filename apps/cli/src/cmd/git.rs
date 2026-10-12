//! `agentplaza git`: Git: repositories, branches, changes.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, confirm, Ctx};

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
    /// The pull requests: open (default), closed or all.
    Pulls {
        /// The repository's name.
        repo: String,
        /// open, closed or all.
        #[arg(long, default_value = "open", value_parser = clap::builder::PossibleValuesParser::new(["open", "closed", "all"]))]
        state: String,
    },
    /// One pull request: its description, files changed and reviews.
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
    /// Merge a pull request.
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
                if g.get("configured").and_then(Value::as_bool).unwrap_or(false) {
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
                ctx::table(&["name", "branch", "open prs", "project", "updated", "description"], &rows);
            });
            Ok(())
        }
        Cmd::Create { name, description } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err("name the repository".into());
            }
            let description = description.filter(|d| !d.trim().is_empty()).map(|d| ctx::text_arg(&d)).transpose()?.unwrap_or_default();
            let repo = ctx.client.post("/v1/git/repos", &json!({ "name": name, "description": description }))?;
            ctx.out(&repo, |r| println!("created repository {}", ctx::s(r, "name")));
            Ok(())
        }
        Cmd::Show { repo } => {
            let info = repo_info(ctx, &repo)?;
            let branches = ctx.client.get(&repo_path(&repo, "/branches"))?;
            let out = json!({ "repo": info, "branches": branches });
            ctx.out(&out, |_| {
                ctx::fields(&[
                    ("name", ctx::s(&info, "name").to_string()),
                    ("description", ctx::s(&info, "description").to_string()),
                    ("default branch", ctx::s(&info, "default_branch").to_string()),
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
        Cmd::Commits { repo, reference, limit } => {
            let path = format!("{}?ref={}&limit={limit}", repo_path(&repo, "/commits"), ctx::seg(&reference));
            let commits = ctx.client.get(&path)?;
            ctx.out(&commits, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|c| {
                        vec![
                            ctx::s(c, "short").to_string(),
                            ctx::s(c, "author").to_string(),
                            ctx::when(&c["at"]),
                            format!("+{} -{}", ctx::text(c, "additions"), ctx::text(c, "deletions")),
                            ctx::s(c, "message").lines().next().unwrap_or("").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["commit", "author", "at", "changes", "message"], &rows);
            });
            Ok(())
        }
        Cmd::Tree { repo, reference, path } => {
            let url = format!("{}?ref={}&path={}", repo_path(&repo, "/tree"), ctx::seg(&reference), ctx::seg(&path));
            let entries = ctx.client.get(&url)?;
            ctx.out(&entries, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|e| vec![ctx::s(e, "kind").to_string(), ctx::text(e, "size"), ctx::s(e, "path").to_string()])
                    .collect();
                ctx::table(&["kind", "size", "path"], &rows);
            });
            Ok(())
        }
        Cmd::File { repo, path, reference } => {
            let url = format!("{}?ref={}&path={}", repo_path(&repo, "/file"), ctx::seg(&reference), ctx::seg(&path));
            let file = ctx.client.get(&url)?;
            ctx.out(&file, |f| {
                if f.get("binary").and_then(Value::as_bool).unwrap_or(false) {
                    println!("{} is a binary file ({} bytes)", ctx::s(f, "path"), ctx::text(f, "size"));
                } else {
                    println!("{}", ctx::s(f, "text"));
                }
            });
            Ok(())
        }
        Cmd::Pulls { repo, state } => {
            let pulls = ctx.client.get(&format!("{}?state={state}", repo_path(&repo, "/pulls")))?;
            ctx.out(&pulls, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::text(p, "number"),
                            ctx::s(p, "state").to_string(),
                            ctx::s(p, "review").to_string(),
                            ctx::s(p, "author").to_string(),
                            format!("{} -> {}", ctx::s(p, "head"), ctx::s(p, "base")),
                            ctx::s(p, "title").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["number", "state", "review", "author", "flow", "title"], &rows);
            });
            Ok(())
        }
        Cmd::Pull { repo, number } => {
            let p = ctx.client.get(&pull_path(&repo, number, ""))?;
            ctx.out(&p, |p| {
                ctx::fields(&[
                    ("number", format!("#{}", ctx::text(p, "number"))),
                    ("title", ctx::text(p, "title")),
                    ("state", ctx::text(p, "state")),
                    ("review", ctx::text(p, "review")),
                    ("author", ctx::text(p, "author")),
                    ("flow", format!("{} -> {}", ctx::s(p, "head"), ctx::s(p, "base"))),
                    ("mergeable", ctx::text(p, "mergeable")),
                    ("updated", ctx::text(p, "updated_at")),
                ]);
                let body = ctx::s(p, "body");
                println!("\n{}", if body.is_empty() { "No description provided." } else { body });
                println!("\nFiles changed ({})", array_at(p, "files").len());
                for f in array_at(p, "files") {
                    println!("  +{} -{}  {}", ctx::text(f, "additions"), ctx::text(f, "deletions"), ctx::s(f, "path"));
                }
                println!("\nReviews ({})", array_at(p, "reviews").len());
                for r in array_at(p, "reviews") {
                    println!("  {} {}: {}", ctx::s(r, "author"), ctx::s(r, "state"), ctx::s(r, "body"));
                }
            });
            Ok(())
        }
        Cmd::Diff { repo, number } => {
            let p = ctx.client.get(&pull_path(&repo, number, ""))?;
            ctx.out(&p, |p| println!("{}", ctx::s(p, "diff")));
            Ok(())
        }
        Cmd::Review { repo, number, event, body } => {
            let event = match event.as_str() {
                "approve" => "APPROVE",
                "request-changes" => "REQUEST_CHANGES",
                _ => "COMMENT",
            };
            let body = body.filter(|b| !b.trim().is_empty()).map(|b| ctx::text_arg(&b)).transpose()?.unwrap_or_default();
            if event != "APPROVE" && body.is_empty() {
                return Err("write the review's text first (--body)".into());
            }
            let r = ctx.client.post(&pull_path(&repo, number, "/review"), &json!({ "event": event, "body": body }))?;
            ctx.out(&r, |_| println!("reviewed #{number} in {repo}: {event}"));
            Ok(())
        }
        Cmd::Merge { repo, number, style, yes } => {
            if !confirm(yes, &format!("Merge #{number} in {repo} ({style})."))? {
                return Err("not done".into());
            }
            let r = ctx.client.post(&pull_path(&repo, number, "/merge"), &json!({ "style": style }))?;
            ctx.out(&r, |_| println!("merged #{number} in {repo} ({style})"));
            Ok(())
        }
    }
}
