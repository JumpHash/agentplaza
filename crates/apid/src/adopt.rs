//! Adopting a project that exists already: a finished (or nearly finished) product the plaza did not build.
//!
//! The owner says where it is, and the plaza opens it, brings in its history, records where it runs and puts it in
//! the mode the owner chose (maintenance unless told otherwise), where its watchers map it first. The source can be:
//! - a repository on a git platform the plaza may use (`platform`: the authorization's id, and `repo`): it is
//!   connected like any other, so later work exports back to it;
//! - a git URL anyone can read (`git`, with `branch`): its history is imported once;
//! - a folder with a git repository on the hub's machine (`path`, with `branch`);
//! - nothing yet: its files come by upload (`project_files`) once it is open.
//!
//! `POST /v1/projects/adopt {title, goal?, kind?, mode?, finished?, source?, environments?}`
//! → 201 `{project, import, environments, sweep}`. `finished` (default true) opens it delivered, else in
//! implementation. `environments` is a list of `{name, kind, url?}` made with it.

use apz_http::{Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::App;

#[derive(Debug, Deserialize)]
struct Source {
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    git: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    /// Register the hub's signing key on the owner's platform account (platform sources).
    #[serde(default)]
    sign: bool,
}

#[derive(Debug, Deserialize)]
struct AdoptBody {
    title: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    finished: Option<bool>,
    #[serde(default)]
    source: Option<Source>,
    #[serde(default)]
    environments: Vec<Value>,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router.post("/v1/projects/adopt", adopt_route).doc(
        RouteDoc::new("Adopt a project that exists already: open it, import its history, record its environments")
            .status(201)
            .errors(&[400, 403, 404, 502, 503]),
    )
}

/// What the source is, checked before anything is made: one of platform+repo, git, path, or none.
#[derive(Debug)]
enum From {
    Platform {
        id: String,
        repo: String,
        sign: bool,
    },
    Git {
        url: String,
        branch: String,
    },
    Path {
        path: String,
        branch: String,
    },
    Nothing,
}

fn source_of(source: Option<Source>) -> Result<From, String> {
    let Some(s) = source else {
        return Ok(From::Nothing);
    };
    let given = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let branch = given(&s.branch).unwrap_or_else(|| "main".into());
    match (given(&s.platform), given(&s.git), given(&s.path)) {
        (Some(id), None, None) => {
            let repo = given(&s.repo).ok_or("a platform source names its repository (repo)")?;
            Ok(From::Platform {
                id,
                repo,
                sign: s.sign,
            })
        }
        (None, Some(url), None) => {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(
                    "a git source is an http(s) URL; for a folder on this machine use path".into(),
                );
            }
            Ok(From::Git { url, branch })
        }
        (None, None, Some(path)) => {
            if !std::path::Path::new(&path).is_absolute() {
                return Err("path is an absolute folder on the hub's machine".into());
            }
            if !std::path::Path::new(&path).exists() {
                return Err(format!("{path} does not exist on the hub's machine"));
            }
            Ok(From::Path { path, branch })
        }
        (None, None, None) => Ok(From::Nothing),
        _ => Err("give one source: platform (with repo), git or path".into()),
    }
}

fn adopt_route(app: &App, req: &Request, _: &apz_http::Params<'_>) -> Response {
    let body: AdoptBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let mode = body.mode.clone().unwrap_or_else(|| "maintenance".into());
    if !crate::projects::is_mode(&mode) {
        return Response::error(400, &format!("unknown mode {mode:?}"));
    }
    let kind = body.kind.clone().unwrap_or_else(|| "code".into());
    let from = match source_of(body.source) {
        Ok(from) => from,
        Err(e) => return Response::error(400, &e),
    };
    if !matches!(from, From::Nothing) && !app.weft.running() {
        return Response::error(
            503,
            "Weft is not running: the history cannot be imported now",
        );
    }
    let goal = if body.goal.trim().is_empty() {
        format!(
            "Look after {}, a product that exists already.",
            body.title.trim()
        )
    } else {
        body.goal.clone()
    };
    let project = match app.projects.open_for_owner(&body.title, &goal, &kind) {
        Ok(p) => p,
        Err(e) => return Response::error(400, &e),
    };
    let id = project.id.clone();
    // From here on, a failure removes nothing: the project stays, open, with what was done, and the answer says what
    // did not work (the owner can connect or upload again).
    let mut problems: Vec<String> = Vec::new();
    if body.finished.unwrap_or(true)
        && let Err(e) = app
            .projects
            .mark_delivered(&id, "adopted: it was finished before the plaza")
    {
        problems.push(e);
    }
    if let Err(e) = app.projects.set_mode(&id, &mode) {
        problems.push(e);
    }
    let import = match &from {
        From::Nothing => {
            if app.weft.running()
                && let Err(e) = crate::weft_hub::ensure_repo(app, &id)
            {
                problems.push(format!("the repository: {e}"));
            }
            Value::Null
        }
        From::Platform {
            id: integ,
            repo,
            sign,
        } => {
            match crate::platforms::connect_project(app, integ, repo, &id, *sign) {
                Ok(v) => v,
                Err((code, e)) if code == 403 || code == 404 => {
                    // Nothing about the platform was right: the project is of no use half made.
                    let _ = app.projects.archive(&id);
                    return Response::error(code, &e);
                }
                Err((_, e)) => {
                    problems.push(format!("connecting {repo}: {e}"));
                    Value::Null
                }
            }
        }
        From::Git { url, branch } | From::Path { path: url, branch } => {
            match crate::weft_hub::ensure_repo(app, &id) {
                Ok(repo) => match app.weft.post(
                    &format!("/repos/{repo}/import"),
                    &json!({ "source": url, "branch": branch, "platform": "git" }),
                ) {
                    Ok(v) => {
                        crate::review_graph::restore_carried(app, &repo);
                        v
                    }
                    Err(e) => {
                        problems.push(format!("importing {url}: {e}"));
                        Value::Null
                    }
                },
                Err(e) => {
                    problems.push(format!("the repository: {e}"));
                    Value::Null
                }
            }
        }
    };
    let mut environments = Vec::new();
    for env in body.environments.iter().take(10) {
        let name = env["name"].as_str().unwrap_or("").to_string();
        match crate::environments::put_for_owner(app, &id, &name, env) {
            Ok(e) => environments.push(e),
            Err(e) => problems.push(format!("environment {name}: {e}")),
        }
    }
    let project = app.projects.get(&id).unwrap_or(project);
    let sweep = if project.in_maintenance() {
        crate::maintenance::entered(app, &id);
        let book = app.maintenance.get(&id);
        json!({ "running": book.sweep.running, "watchers": crate::maintenance::watchers(app, &id, &book.config) })
    } else {
        Value::Null
    };
    app.bus.publish("project", &project);
    Response::json(
        201,
        &json!({ "project": project, "import": import, "environments": environments, "sweep": sweep, "problems": problems }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(v: Value) -> Result<From, String> {
        source_of(Some(serde_json::from_value(v).unwrap()))
    }

    #[test]
    fn a_source_is_one_thing() {
        assert!(matches!(source_of(None), Ok(From::Nothing)));
        assert!(matches!(
            src(json!({"git": "https://example.com/a/b.git"})),
            Ok(From::Git { branch, .. }) if branch == "main"
        ));
        assert!(matches!(
            src(json!({"platform": "gh1", "repo": "acme/shop"})),
            Ok(From::Platform { repo, .. }) if repo == "acme/shop"
        ));
        assert!(
            src(json!({"platform": "gh1"}))
                .unwrap_err()
                .contains("repo")
        );
        assert!(
            src(json!({"git": "git@github.com:a/b.git"}))
                .unwrap_err()
                .contains("http")
        );
        assert!(
            src(json!({"path": "relative/dir"}))
                .unwrap_err()
                .contains("absolute")
        );
        assert!(
            src(json!({"git": "https://example.com/x", "path": "/tmp"}))
                .unwrap_err()
                .contains("one source")
        );
    }
}
