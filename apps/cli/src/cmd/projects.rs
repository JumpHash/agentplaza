//! `agentplaza projects`: Projects. The owner opens one (its plan theirs), pauses, resumes and archives it, and sets
//! its settings and budget; the agents' plans go through their stages, which the owner approves or sends back. A
//! project is in a mode: build, or maintenance, where its watchers keep it. The submodules hold the settings and the
//! budget (`settings`), the maintenance (`maintenance`), the environments (`environments`) and the files of its
//! repository (`files`).

pub mod environments;
pub mod files;
pub mod maintenance;
pub mod settings;

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm};

use super::tasks::title_of;

/// The stages a project goes through, in order (the pipeline on the Projects screen).
const STAGES: [&str; 8] = [
    "brief",
    "research",
    "proposal",
    "critique",
    "approval",
    "implementation",
    "review",
    "delivered",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them, with their stage and whether they are active, paused, archived or delivered.
    List,
    /// One project in full: its goal and state, its tasks, its documents, critiques and history.
    Show {
        /// The project's id.
        id: String,
    },
    /// Open a project as the owner: its plan is yours, so it is at implementation at once and takes tasks.
    /// `agentplaza chats` talks a project into being with its planner: the draft's start card in the inbox starts it.
    Create {
        /// The project's title.
        title: String,
        /// What it is for: the goal its tasks work towards. `-` reads it from stdin.
        #[arg(long)]
        goal: String,
        /// `code` (it gets a repository; the hub's default) or `other`.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["code", "other"]))]
        kind: Option<String>,
    },
    /// Pause a project: its agents' work waits until it is resumed.
    Pause {
        /// The project's id.
        id: String,
    },
    /// Resume a paused project: its agents' waiting work goes out.
    Resume {
        /// The project's id.
        id: String,
    },
    /// Archive a project for good: its open tasks are withdrawn, and the rest of its budget goes back to the plaza.
    /// Asks to confirm unless --yes.
    Archive {
        /// The project's id.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// A project's settings: the plaza's settings it overrides for its agents. Add `effective` for every value they
    /// get and where it comes from, or `set KEY=VALUE…` and `unset KEY…` to change them.
    Settings {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<settings::Action>,
    },
    /// A project's budget: credits set aside from the plaza's budget for its tasks. Add `add CREDITS` to set more
    /// aside, or `return CREDITS` to give some back.
    Budget {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<settings::BudgetAction>,
    },
    /// Approve the project's plan (the owner's approval), with an optional note.
    Approve {
        /// The project's id.
        id: String,
        /// A note for the team.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Send a project back to an earlier stage, with the reason the team must act on.
    SendBack {
        /// The project's id.
        id: String,
        /// The stage it returns to (an earlier one than its current stage).
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(STAGES))]
        stage: String,
        /// What must change, and why (required).
        #[arg(long)]
        reason: String,
    },
    /// Put a project in a mode: `build` (its plan is worked as usual, and a delivered project reopens at
    /// implementation) or `maintenance` (its watchers keep it; see `maintenance`).
    Mode {
        /// The project's id.
        id: String,
        /// `build` or `maintenance`.
        #[arg(value_parser = clap::builder::PossibleValuesParser::new(["build", "maintenance"]))]
        mode: String,
    },
    /// The modes a project can be in, and what each one means.
    Modes,
    /// Adopt a project that exists already: open it (delivered, unless --unfinished), import its history from a git URL,
    /// a platform's repository or a folder on the hub, record its environments and put it in maintenance (unless
    /// --mode says otherwise).
    Adopt {
        /// The project's title.
        title: String,
        /// What it is for.
        #[arg(long)]
        goal: Option<String>,
        /// `code` (the default) or `other`.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["code", "other"]))]
        kind: Option<String>,
        /// `maintenance` (the default) or `build`.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["maintenance", "build"]))]
        mode: Option<String>,
        /// The project is still in progress: it opens in implementation, not delivered.
        #[arg(long)]
        unfinished: bool,
        /// A git URL anyone can read, to import the history from.
        #[arg(long, value_name = "URL", conflicts_with_all = ["path", "platform"])]
        git: Option<String>,
        /// A folder with a git repository on the hub's machine, as an absolute path.
        #[arg(long, value_name = "DIR", conflicts_with_all = ["git", "platform"])]
        path: Option<String>,
        /// The id of a git platform's authorization (see `agentplaza platforms`), whose repository --repo names.
        #[arg(long, value_name = "ID", requires = "repo", conflicts_with_all = ["git", "path"])]
        platform: Option<String>,
        /// The repository on that platform, as OWNER/NAME.
        #[arg(long, value_name = "OWNER/NAME", requires = "platform")]
        repo: Option<String>,
        /// Also register the hub's signing key on the platform account.
        #[arg(long, requires = "platform")]
        sign: bool,
        /// The branch to import, with --git or --path.
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,
        /// An environment as NAME=KIND[,URL], such as production=production,https://shop.example; repeat for more.
        #[arg(long = "env", value_name = "NAME=KIND[,URL]")]
        env: Vec<String>,
    },
    /// A project's maintenance: its focus and schedule, its last sweep, the signals its watchers read, the map of its
    /// areas and the proposals waiting for the owner. Add a subcommand to sweep, change the focus or schedule, record a
    /// signal, decide a proposal or open an ingest address.
    Maintenance {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<maintenance::Action>,
    },
    /// A project's environments: where it runs, with each one's kind, address, track and version. Add `set`, `rm`,
    /// `deploy` or `secret` to change them.
    Env {
        /// The project's id.
        id: String,
        #[command(subcommand)]
        action: Option<environments::Action>,
    },
    /// Upload files and folders into a project's repository, as one change of the owner's. A folder keeps its own name
    /// as the first part of its files' paths. .git, node_modules and .DS_Store are left out.
    Upload {
        /// The project's id.
        id: String,
        /// The files and folders to add.
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// A folder in the repository the files go into.
        #[arg(long)]
        folder: Option<String>,
        /// The change's title.
        #[arg(long)]
        title: Option<String>,
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

/// Where a project stands for its agents: `archived` or `delivered` (it is over), `maintenance` (it is delivered and
/// kept up by its watchers), `paused` (their work waits), or `active`.
pub(crate) fn state_of(p: &Value) -> &'static str {
    let flag = |key: &str| p.get(key).and_then(Value::as_bool).unwrap_or(false);
    if flag("archived") {
        "archived"
    } else if ctx::s(p, "stage") == "delivered" {
        if ctx::s(p, "mode") == "maintenance" {
            "maintenance"
        } else {
            "delivered"
        }
    } else if flag("paused") {
        "paused"
    } else {
        "active"
    }
}

/// The `POST /v1/projects` body: the title and goal, and the kind only when one is given (the hub's default is code).
fn create_body(title: &str, goal: &str, kind: Option<&str>) -> Value {
    let mut body = json!({ "title": title, "goal": goal });
    if let Some(kind) = kind {
        body["kind"] = json!(kind);
    }
    body
}

/// The text of a field shown in a list: a dash when it is empty.
fn dash(text: String) -> String {
    if text.is_empty() {
        "-".to_string()
    } else {
        text
    }
}

/// The `source` of an adoption: a platform's repository (with `sign` when asked), a git URL or a folder on the hub
/// (with the `branch`, when one is given), or none, when the files come later by upload.
fn source_of(
    git: Option<&str>,
    path: Option<&str>,
    platform: Option<&str>,
    repo: Option<&str>,
    sign: bool,
    branch: Option<&str>,
) -> Result<Option<Value>, String> {
    let given = [git.is_some(), path.is_some(), platform.is_some()]
        .into_iter()
        .filter(|given| *given)
        .count();
    if given > 1 {
        return Err("give one source: --git, --path, or --platform with --repo".into());
    }
    if let Some(platform) = platform {
        let repo = repo.ok_or_else(|| "--platform needs --repo OWNER/NAME".to_string())?;
        if branch.is_some() {
            return Err(
                "--branch goes with --git or --path; a platform repository has its own".into(),
            );
        }
        let mut source = json!({ "platform": platform, "repo": repo });
        if sign {
            source["sign"] = json!(true);
        }
        return Ok(Some(source));
    }
    if repo.is_some() || sign {
        return Err("--repo and --sign go with --platform".into());
    }
    let mut source = match (git, path) {
        (Some(url), _) => json!({ "git": url }),
        (None, Some(dir)) => json!({ "path": dir }),
        (None, None) => return Ok(None),
    };
    if let Some(branch) = branch {
        source["branch"] = json!(branch);
    }
    Ok(Some(source))
}

/// The `POST /v1/projects/adopt` body: the title (required), the goal, kind and mode when given, whether the project is
/// finished (a finished one opens delivered), its source, and its environments.
fn adopt_body(
    title: &str,
    goal: Option<&str>,
    kind: Option<&str>,
    mode: Option<&str>,
    finished: bool,
    source: Option<Value>,
    environments: &[Value],
) -> Result<Value, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("an adopted project needs a title".into());
    }
    let mut body = json!({ "title": title, "finished": finished });
    if let Some(goal) = goal.map(str::trim).filter(|goal| !goal.is_empty()) {
        body["goal"] = json!(goal);
    }
    if let Some(kind) = kind {
        body["kind"] = json!(kind);
    }
    if let Some(mode) = mode {
        body["mode"] = json!(mode);
    }
    if let Some(source) = source {
        body["source"] = source;
    }
    if !environments.is_empty() {
        body["environments"] = json!(environments);
    }
    Ok(body)
}

/// What an adoption made, in words: the project, what its import brought in (when the import counts commits), its
/// environments, and each problem on a line of its own.
fn print_adopted(a: &Value) {
    let p = &a["project"];
    println!(
        "adopted project {}: stage {}, mode {}",
        ctx::s(p, "id"),
        ctx::s(p, "stage"),
        ctx::s(p, "mode")
    );
    if let Some(commits) = a["import"]["commits"].as_u64() {
        println!("imported {commits} changes");
    }
    let names: Vec<&str> = array_at(a, "environments")
        .iter()
        .map(|e| ctx::s(e, "name"))
        .collect();
    if !names.is_empty() {
        println!("environments: {}", names.join(", "));
    }
    for problem in array_at(a, "problems").iter().filter_map(Value::as_str) {
        println!("problem: {problem}");
    }
}

/// The project's tasks in its order. The project lists them by id only, so each is joined with
/// the task list (its state and goal); an id the list does not show stays a bare id.
fn task_rows(project_tasks: &[Value], all_tasks: &[Value]) -> Vec<Value> {
    project_tasks
        .iter()
        .map(|t| {
            let id = t.as_str().unwrap_or_default();
            all_tasks
                .iter()
                .find(|task| ctx::s(task, "id") == id)
                .cloned()
                .unwrap_or_else(|| json!({ "id": id }))
        })
        .collect()
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let projects = ctx.client.get("/v1/projects")?;
            ctx.out(&projects, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::s(p, "id").to_string(),
                            ctx::s(p, "title").to_string(),
                            ctx::s(p, "kind").to_string(),
                            ctx::s(p, "stage").to_string(),
                            state_of(p).to_string(),
                            ctx::text(p, "repo"),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "title", "kind", "stage", "state", "repo"], &rows);
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let p = ctx.client.get(&format!("/v1/projects/{}", ctx::seg(&id)))?;
            // The JSON is the hub's project as it is; the task list is read only for the table view.
            let tasks = if ctx.json {
                Vec::new()
            } else {
                let all = ctx.client.get("/v1/tasks")?;
                task_rows(array_at(&p, "tasks"), array_at(&all, "tasks"))
            };
            ctx.out(&p, |p| {
                ctx::fields(&[
                    ("id", ctx::text(p, "id")),
                    ("title", ctx::text(p, "title")),
                    ("kind", ctx::text(p, "kind")),
                    ("stage", ctx::text(p, "stage")),
                    ("state", state_of(p).to_string()),
                    ("repo", ctx::text(p, "repo")),
                    ("approved by", ctx::text(p, "approved_by")),
                    ("revision", ctx::text(p, "revision")),
                ]);
                println!("\n{}", ctx::s(p, "goal"));
                println!("\nTasks ({})", array_at(p, "tasks").len());
                for t in &tasks {
                    let goal = ctx::s(t, "goal");
                    let title = if goal.is_empty() {
                        "(not in the task list)"
                    } else {
                        title_of(goal)
                    };
                    println!("  {}  {}  {}", ctx::s(t, "id"), ctx::s(t, "state"), title);
                }
                println!("\nDocuments ({})", array_at(p, "docs").len());
                for d in array_at(p, "docs") {
                    // The file's id is what `library download` takes; a document without a file has none.
                    let file = ctx::text(d, "file");
                    let file = if file.is_empty() {
                        "-".to_string()
                    } else {
                        file
                    };
                    println!(
                        "  {}  {}  file {}: {}",
                        ctx::s(d, "kind"),
                        ctx::s(d, "title"),
                        file,
                        ctx::s(d, "summary")
                    );
                }
                println!("\nCritiques ({})", array_at(p, "critiques").len());
                for c in array_at(p, "critiques") {
                    println!(
                        "  revision {} {} by {}: {}",
                        ctx::text(c, "revision"),
                        ctx::s(c, "verdict"),
                        ctx::s(c, "by"),
                        ctx::s(c, "text")
                    );
                }
                println!("\nHistory");
                for e in array_at(p, "history") {
                    let note = ctx::s(e, "note");
                    let note = if note.is_empty() {
                        String::new()
                    } else {
                        format!(" ({note})")
                    };
                    println!(
                        "  {}  {} {}{}",
                        ctx::when(&e["at_ms"]),
                        ctx::s(e, "by"),
                        ctx::s(e, "what"),
                        note
                    );
                }
            });
            Ok(())
        }
        Cmd::Create { title, goal, kind } => {
            let title = title.trim();
            if title.is_empty() {
                return Err("a project needs a title".into());
            }
            let goal = ctx::text_arg(&goal)?;
            let p = ctx
                .client
                .post("/v1/projects", &create_body(title, &goal, kind.as_deref()))?;
            ctx.out(&p, |p| {
                let id = ctx::s(p, "id");
                println!("opened project {id} ({})", ctx::s(p, "stage"));
            });
            Ok(())
        }
        Cmd::Pause { id } => lifecycle(ctx, &id, "pause", "paused project"),
        Cmd::Resume { id } => lifecycle(ctx, &id, "resume", "resumed project"),
        Cmd::Archive { id, yes } => {
            let question = format!(
                "Archive project {id}? Its open tasks are withdrawn, and the rest of its budget goes back to the plaza."
            );
            if !confirm(yes, &question)? {
                println!("cancelled");
                return Ok(());
            }
            lifecycle(ctx, &id, "archive", "archived project")
        }
        Cmd::Settings { id, action } => settings::settings(ctx, &id, action),
        Cmd::Budget { id, action } => settings::budget(ctx, &id, action),
        Cmd::Approve { id, note } => {
            let p = ctx.client.post(
                &format!("/v1/projects/{}/approve", ctx::seg(&id)),
                &json!({ "note": note }),
            )?;
            ctx.out(&p, |_| println!("approved the plan of project {id}"));
            Ok(())
        }
        Cmd::SendBack { id, stage, reason } => {
            if reason.trim().is_empty() {
                return Err("give a reason so the team knows what to change".into());
            }
            let body = json!({ "stage": stage, "reason": reason.trim() });
            let p = ctx
                .client
                .post(&format!("/v1/projects/{}/veto", ctx::seg(&id)), &body)?;
            ctx.out(&p, |_| println!("sent project {id} back to {stage}"));
            Ok(())
        }
        Cmd::Mode { id, mode } => {
            let p = ctx.client.post(
                &format!("/v1/projects/{}/mode", ctx::seg(&id)),
                &json!({ "mode": mode }),
            )?;
            ctx.out(&p, |_| match mode.as_str() {
                "maintenance" => println!("{id} is in maintenance now"),
                _ => {
                    println!("{id} is in build now; a delivered project reopens at implementation")
                }
            });
            Ok(())
        }
        Cmd::Modes => {
            let modes = ctx.client.get("/v1/project-modes")?;
            ctx.out(&modes, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|m| {
                        vec![
                            ctx::s(m, "id").to_string(),
                            ctx::s(m, "label").to_string(),
                            ctx::s(m, "summary").to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "label", "summary"], &rows);
            });
            Ok(())
        }
        Cmd::Adopt {
            title,
            goal,
            kind,
            mode,
            unfinished,
            git,
            path,
            platform,
            repo,
            sign,
            branch,
            env,
        } => {
            let source = source_of(
                git.as_deref(),
                path.as_deref(),
                platform.as_deref(),
                repo.as_deref(),
                sign,
                branch.as_deref(),
            )?;
            let envs = env
                .iter()
                .map(|spec| environments::parse_env_spec(spec))
                .collect::<Result<Vec<_>, _>>()?;
            let body = adopt_body(
                &title,
                goal.as_deref(),
                kind.as_deref(),
                mode.as_deref(),
                !unfinished,
                source,
                &envs,
            )?;
            let adopted = ctx.client.post("/v1/projects/adopt", &body)?;
            ctx.out(&adopted, print_adopted);
            Ok(())
        }
        Cmd::Maintenance { id, action } => maintenance::run(ctx, &id, action),
        Cmd::Env { id, action } => environments::run(ctx, &id, action),
        Cmd::Upload {
            id,
            paths,
            folder,
            title,
        } => files::run(ctx, &id, &paths, folder.as_deref(), title.as_deref()),
    }
}

/// Pauses, resumes or archives project `id` (`verb` is the route's last segment), and says so with `done`.
fn lifecycle(ctx: &Ctx, id: &str, verb: &str, done: &str) -> Result<(), String> {
    let p = ctx
        .client
        .post(&format!("/v1/projects/{}/{verb}", ctx::seg(id)), &json!({}))?;
    ctx.out(&p, |_| println!("{done} {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_active_paused_or_over() {
        assert_eq!(state_of(&json!({ "stage": "implementation" })), "active");
        assert_eq!(
            state_of(&json!({ "stage": "implementation", "paused": true })),
            "paused"
        );
        assert_eq!(
            state_of(&json!({ "stage": "delivered", "paused": true })),
            "delivered"
        );
        assert_eq!(
            state_of(&json!({ "stage": "review", "paused": true, "archived": true })),
            "archived"
        );
    }

    #[test]
    fn create_names_the_kind_only_when_one_is_given() {
        assert_eq!(
            create_body("Release 0.9", "Tag it", None),
            json!({ "title": "Release 0.9", "goal": "Tag it" })
        );
        assert_eq!(
            create_body("Notes", "Write them", Some("other"))["kind"],
            "other"
        );
    }

    #[test]
    fn a_delivered_project_in_maintenance_reads_as_maintenance() {
        assert_eq!(
            state_of(&json!({ "stage": "delivered", "mode": "maintenance" })),
            "maintenance"
        );
        assert_eq!(
            state_of(&json!({ "stage": "delivered", "mode": "build" })),
            "delivered"
        );
        assert_eq!(state_of(&json!({ "stage": "delivered" })), "delivered");
        assert_eq!(
            state_of(&json!({ "stage": "delivered", "mode": "maintenance", "archived": true })),
            "archived"
        );
        assert_eq!(
            state_of(&json!({ "stage": "implementation", "mode": "maintenance" })),
            "active"
        );
    }

    #[test]
    fn an_adoption_has_one_source_of_each_kind() {
        assert_eq!(
            source_of(None, None, Some("gh1"), Some("acme/shop"), true, None).unwrap(),
            Some(json!({ "platform": "gh1", "repo": "acme/shop", "sign": true }))
        );
        assert_eq!(
            source_of(
                Some("https://example.com/a.git"),
                None,
                None,
                None,
                false,
                Some("dev")
            )
            .unwrap(),
            Some(json!({ "git": "https://example.com/a.git", "branch": "dev" }))
        );
        assert_eq!(
            source_of(None, Some("/srv/shop"), None, None, false, None).unwrap(),
            Some(json!({ "path": "/srv/shop" }))
        );
        assert_eq!(
            source_of(None, None, None, None, false, None).unwrap(),
            None
        );
    }

    #[test]
    fn an_adoption_refuses_mixed_or_incomplete_sources() {
        assert!(source_of(Some("https://x"), Some("/y"), None, None, false, None).is_err());
        assert!(
            source_of(None, None, Some("gh1"), None, false, None).is_err(),
            "a platform needs its repo"
        );
        assert!(source_of(None, None, Some("gh1"), Some("a/b"), false, Some("main")).is_err());
        assert!(source_of(None, None, None, Some("a/b"), false, None).is_err());
    }

    #[test]
    fn an_adoption_body_says_what_the_owner_gave() {
        let envs = [json!({ "name": "production", "kind": "production" })];
        let body = adopt_body(
            "Shop",
            Some(" the storefront "),
            Some("code"),
            None,
            true,
            None,
            &envs,
        )
        .unwrap();
        assert_eq!(
            body,
            json!({
                "title": "Shop", "goal": "the storefront", "kind": "code", "finished": true,
                "environments": [{ "name": "production", "kind": "production" }]
            })
        );
        let unfinished = adopt_body(
            "Shop",
            None,
            None,
            Some("build"),
            false,
            Some(json!({ "git": "u" })),
            &[],
        )
        .unwrap();
        assert_eq!(
            unfinished,
            json!({ "title": "Shop", "finished": false, "mode": "build", "source": { "git": "u" } })
        );
        assert!(adopt_body("  ", None, None, None, true, None, &[]).is_err());
    }
}
