//! A project's test topologies (PLAN 22.6): `.plaza/topologies/*.toml` at main in its Weft repo, read the way the build
//! declaration is read (project_stack.rs). The lab runs them (apz-topology: one lab VM per topology).
//!
//! Route: `GET /v1/projects/{id}/topologies`: each file with its typed topology, or the reason it does not parse.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_topology::spec::{self, DIR, Topology};
use serde_json::{Value, json};

use crate::state::App;

/// One topology file of a project: its repo path, and its topology or the reason it is not one.
pub struct Found {
    pub path: String,
    pub topology: Result<Topology, String>,
}

/// The topology files in a revision's file listing, each read with `fetch` (pure over the listing; the files come back
/// sorted by path).
pub fn in_listing(listing: &Value, fetch: impl Fn(&str) -> Result<Vec<u8>, String>) -> Vec<Found> {
    let prefix = format!("{DIR}/");
    let mut paths: Vec<&str> = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["path"].as_str())
        .filter(|path| path.starts_with(prefix.as_str()) && path.ends_with(".toml"))
        .collect();
    paths.sort_unstable();
    paths
        .into_iter()
        .map(|path| {
            let topology = fetch(path).and_then(|bytes| {
                let text = String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))?;
                spec::parse(&text).map_err(|why| format!("{path}: {why}"))
            });
            Found {
                path: path.to_string(),
                topology,
            }
        })
        .collect()
}

/// The topology files of `project` at its repo's main (none when the project has no repo yet).
pub fn read(app: &App, project: &str) -> Result<Vec<Found>, String> {
    let Some(repo) = app.projects.get(project).and_then(|p| p.repo) else {
        return Ok(Vec::new());
    };
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?;
    let listing = app.weft.get(&format!("/revisions/{rev}/files"))?;
    let client = app.weft.client("hub")?;
    Ok(in_listing(&listing, |path| {
        client.get_bytes(&format!("/revisions/{rev}/file?path={path}"))
    }))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/topologies", topologies_route)
        .doc(
            RouteDoc::new(
                "A project's test topologies: each .plaza/topologies/*.toml at main, typed, or the reason it does not parse",
            )
            .errors(&[404, 502]),
        )
}

/// `GET /v1/projects/{id}/topologies`.
fn topologies_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    match read(app, project) {
        Ok(found) => Response::json(
            200,
            &json!({"topologies": found.iter().map(found_json).collect::<Vec<Value>>()}),
        ),
        Err(why) => Response::error(502, &why),
    }
}

fn found_json(found: &Found) -> Value {
    match &found.topology {
        Ok(topology) => json!({"path": found.path, "topology": topology, "error": null}),
        Err(why) => json!({"path": found.path, "topology": null, "error": why}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_STACK: &str =
        "name = \"web-stack\"\n[[service]]\nname = \"api\"\nimage = \"postgres:17\"\n";

    fn listing() -> Value {
        json!({"files": [
            {"path": ".plaza/topologies/web-stack.toml"},
            {"path": ".plaza/topologies/broken.toml"},
            {"path": ".plaza/topologies/notes.md"},
            {"path": ".plaza/memory/entry.md"},
        ]})
    }

    fn fetch(path: &str) -> Result<Vec<u8>, String> {
        match path {
            ".plaza/topologies/web-stack.toml" => Ok(WEB_STACK.as_bytes().to_vec()),
            ".plaza/topologies/broken.toml" => Ok(b"name = 3".to_vec()),
            other => Err(format!("unexpected read of {other}")),
        }
    }

    #[test]
    fn only_topology_files_are_read_and_each_is_typed_or_refused_with_its_path() {
        let found = in_listing(&listing(), fetch);
        let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".plaza/topologies/broken.toml",
                ".plaza/topologies/web-stack.toml"
            ]
        );
        let why = found[0].topology.as_ref().unwrap_err();
        assert!(why.starts_with(".plaza/topologies/broken.toml: "), "{why}");
        assert_eq!(found[1].topology.as_ref().unwrap().name, "web-stack");
    }

    #[test]
    fn a_file_that_cannot_be_fetched_is_an_error_for_that_file_only() {
        let listing = json!({"files": [{"path": ".plaza/topologies/gone.toml"}]});
        let found = in_listing(&listing, |_| Err("404".to_string()));
        assert_eq!(found[0].topology.as_ref().unwrap_err(), "404");
    }
}
