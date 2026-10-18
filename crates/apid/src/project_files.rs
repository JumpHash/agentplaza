//! Files the owner brings into a project's repository: dropped on the console or picked in a file browser, while the
//! project is made or any time after.
//!
//! The console sends each file's bytes on their own (`PUT /v1/projects/{id}/uploads`, the raw body), which go into
//! Weft as objects by hash (chunked past 4 MiB, so nothing large travels twice); then one call lands them all as one
//! owner's change on main (`POST /v1/projects/{id}/files {title?, folder?, files: [{path, hash}]}`). An object that is
//! never named in a change is only an unused blob.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use crate::state::App;

/// One file's bytes at most.
const MAX_FILE: usize = 200 << 20;
/// The files one change may bring.
const MAX_FILES: usize = 5000;

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .route("PUT", "/v1/projects/{id}/uploads", upload_route)
        .doc(
            RouteDoc::new("Upload one file's bytes for a project (the raw body); answers its hash")
                .status(201)
                .errors(&[400, 404, 413, 503]),
        )
        .post("/v1/projects/{id}/files", files_route)
        .doc(
            RouteDoc::new(
                "Add uploaded files to a project's repository, as one change of the owner's",
            )
            .errors(&[400, 404, 409, 503]),
        )
}

/// The repository of project `id` in Weft (made when it has none yet).
fn repo_of(app: &App, id: &str) -> Result<String, Response> {
    if app.projects.get(id).is_none() {
        return Err(Response::error(404, "no such project"));
    }
    if !app.weft.running() {
        return Err(Response::error(503, "Weft is not running"));
    }
    crate::weft_hub::ensure_repo(app, id).map_err(|e| Response::error(503, &e))
}

fn upload_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    if let Err(r) = repo_of(app, p.get("id")) {
        return r;
    }
    if req.body.len() > MAX_FILE {
        return Response::error(413, &format!("a file is at most {} MiB", MAX_FILE >> 20));
    }
    let bytes = req.body.clone();
    let hash = weft::hash::Hash::of(&bytes);
    let uploaded = app
        .weft
        .client("owner")
        .and_then(|c| c.upload(&[hash], &|_| Ok(bytes.clone())));
    match uploaded {
        Ok(_) => Response::json(201, &json!({ "hash": hash.hex(), "size": bytes.len() })),
        Err(e) => Response::error(503, &format!("Weft: {e}")),
    }
}

/// A repository path from what the console sent: `/`-separated, relative, no `.` or `..` parts, nothing under `.git`.
pub fn clean_path(folder: &str, path: &str) -> Result<String, String> {
    let joined = format!("{}/{}", folder.trim_matches('/'), path.replace('\\', "/"));
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err(format!("{path}: a path cannot climb out with ..")),
            ".git" => return Err(format!("{path}: nothing goes inside .git")),
            p if p.chars().any(|c| c.is_control()) => {
                return Err(format!("{path}: a path has no control characters"));
            }
            p => parts.push(p),
        }
    }
    let out = parts.join("/");
    if out.is_empty() {
        return Err("a file needs a name".into());
    }
    if out.len() > 400 {
        return Err(format!("{out}: a path is at most 400 characters"));
    }
    Ok(out)
}

fn files_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let repo = match repo_of(app, p.get("id")) {
        Ok(repo) => repo,
        Err(r) => return r,
    };
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let folder = body["folder"].as_str().unwrap_or("");
    let Some(list) = body["files"].as_array().filter(|l| !l.is_empty()) else {
        return Response::error(400, "files is a list of {path, hash}");
    };
    if list.len() > MAX_FILES {
        return Response::error(400, &format!("at most {MAX_FILES} files at a time"));
    }
    let mut files = Vec::with_capacity(list.len());
    let mut seen = std::collections::BTreeSet::new();
    for f in list {
        let path = match clean_path(folder, f["path"].as_str().unwrap_or("")) {
            Ok(path) => path,
            Err(e) => return Response::error(400, &e),
        };
        if !seen.insert(path.clone()) {
            return Response::error(400, &format!("{path} is given twice"));
        }
        let Some(hash) = f["hash"]
            .as_str()
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        else {
            return Response::error(400, &format!("{path}: give the hash its upload answered"));
        };
        files.push(json!({ "path": path, "hash": hash }));
    }
    let n = files.len();
    let title = match body["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(t) => t.chars().take(200).collect(),
        None if n == 1 => format!("Add {}", files[0]["path"].as_str().unwrap_or("a file")),
        None => format!("Add {n} files"),
    };
    let edit = json!({ "title": title, "ref": "main", "files": files });
    match app
        .weft
        .client("owner")
        .and_then(|c| c.post(&format!("/repos/{repo}/edits"), &edit))
    {
        Ok(landed) => {
            app.bus.publish(
                "weft_land",
                &json!({ "repo": repo, "change": landed["change"], "kind": "edit" }),
            );
            Response::json(
                200,
                &json!({ "repo": repo, "files": n, "title": title, "landed": landed }),
            )
        }
        Err(e) if e.contains("409") || e.contains("clash") => Response::error(409, &e),
        Err(e) => Response::error(503, &format!("Weft: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_and_stay_in_the_repository() {
        assert_eq!(clean_path("", "src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(clean_path("/docs/", "./a//b.md").unwrap(), "docs/a/b.md");
        assert_eq!(clean_path("", "dir\\win.txt").unwrap(), "dir/win.txt");
        assert!(clean_path("", "../etc/passwd").is_err());
        assert!(clean_path("x", "a/../../b").is_err());
        assert!(clean_path("", ".git/config").is_err());
        assert!(clean_path("", "").is_err());
        assert!(clean_path("", "a\u{0}b").is_err());
    }
}
