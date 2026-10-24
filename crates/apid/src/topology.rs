//! A project's test topologies (PLAN 22.6): `.plaza/topologies/*.toml` at main in its Weft repo, read the way the build
//! declaration is read (project_stack.rs). The lab runs them (apz-topology: one lab VM per topology).
//!
//! Routes: `GET /v1/projects/{id}/topologies`: each file with its typed topology, or the reason it does not parse.
//! `POST /v1/projects/{id}/topologies/import`: a project's Compose file or rendered manifests converted (apz-topology's
//! `compose` and `manifests`) and landed as `.plaza/topologies/<name>.toml` (a k3s topology also takes its manifest).

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_topology::report::{Conversion, Outcome, note};
use apz_topology::spec::{self, DIR, K3s, Kind, Topology};
use apz_topology::{compose, manifests};
use serde_json::{Value, json};

use crate::project_files::clean_path;
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
        .post("/v1/projects/{id}/topologies/import", import_route)
        .doc(
            RouteDoc::new(
                "Import a project's Compose file or rendered manifests into .plaza/topologies/ as one change of the owner's (dry_run: convert and answer only). Answers the typed topology and a report item per thing that did not carry over",
            )
            .errors(&[400, 404, 409, 502, 503]),
        )
}

/// The topology a source converts to, and the manifest files it needs in the repository beside its TOML (a k3s topology
/// keeps its manifest there). Pure: the caller reads the source and lands the result.
fn convert_source(
    source: &str,
    kind: &str,
    name_arg: Option<String>,
    body: &Value,
    path: &str,
    text: &str,
) -> Result<(Conversion, Vec<(String, Vec<u8>)>), (u16, String)> {
    let bad = |why: String| (400, why);
    Ok(match (source, kind) {
        ("compose", "services") => (
            compose::convert(text, name_arg.as_deref()).map_err(bad)?,
            Vec::new(),
        ),
        ("compose", _) => return Err(bad("a Compose file converts to a services topology".into())),
        ("manifests", "services") => {
            let name = name_arg
                .ok_or_else(|| bad("a manifests import needs the topology's name".into()))?;
            (manifests::convert(text, &name).map_err(bad)?, Vec::new())
        }
        ("manifests", "k3s") => {
            let name = name_arg
                .ok_or_else(|| bad("a manifests import needs the topology's name".into()))?;
            let version = body["k3s_version"].as_str().ok_or_else(|| {
                bad("a k3s import needs k3s_version, a pinned release such as v1.31.4+k3s1".into())
            })?;
            let file = path.rsplit('/').next().unwrap_or(path);
            let manifest = format!("{DIR}/{name}/manifests/{file}");
            let topology = Topology {
                name: name.clone(),
                kind: Kind::K3s,
                services: Vec::new(),
                machines: Vec::new(),
                k3s: Some(K3s {
                    version: version.to_string(),
                    manifests: vec![manifest.clone()],
                    images: manifests::images(text).map_err(bad)?,
                }),
                observe: Default::default(),
                traffic: None,
                assertions: Vec::new(),
                resilience: Vec::new(),
                egress: Default::default(),
                data: None,
                cache: None,
                messaging: None,
                security: Default::default(),
            };
            topology.check().map_err(bad)?;
            let mut report = Vec::new();
            note(
                &mut report,
                path.to_string(),
                Outcome::Converted,
                format!("applied by k3s when the cluster starts, from {manifest}"),
            );
            (
                Conversion { topology, report },
                vec![(manifest, text.as_bytes().to_vec())],
            )
        }
        ("manifests", other) => {
            return Err(bad(format!(
                "kind is \"services\" or \"k3s\", not {other:?}"
            )));
        }
        (other, _) => {
            return Err(bad(format!(
                "source is \"compose\" or \"manifests\", not {other:?}"
            )));
        }
    })
}

/// The bytes of `path` in the project's repository at main.
fn read_file(app: &App, project: &str, path: &str) -> Result<Vec<u8>, (u16, String)> {
    let Some(repo) = app.projects.get(project).and_then(|p| p.repo) else {
        return Err((404, format!("{path}: the project has no repository yet")));
    };
    let detail = app
        .weft
        .get(&format!("/repos/{repo}"))
        .map_err(|e| (503, format!("Weft: {e}")))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| (404, format!("{repo} has no main")))?;
    let client = app
        .weft
        .client("hub")
        .map_err(|e| (503, format!("Weft: {e}")))?;
    client
        .get_bytes(&format!("/revisions/{rev}/file?path={path}"))
        .map_err(|e| {
            if e.contains("404") {
                (404, format!("{path} is not on main"))
            } else {
                (502, format!("Weft: {e}"))
            }
        })
}

/// `POST /v1/projects/{id}/topologies/import`.
fn import_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    let body: Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    match import(app, project, &body) {
        Ok(out) => Response::json(200, &out),
        Err((status, why)) => Response::error(status, &why),
    }
}

/// The import `body` asks for: its source converted, the topology written back as TOML and read again (so the file is one
/// `spec::parse` takes), and landed as one change on main unless `dry_run`. Errors are a status and its reason.
fn import(app: &App, project: &str, body: &Value) -> Result<Value, (u16, String)> {
    let bad = |why: String| (400, why);
    let source = body["source"]
        .as_str()
        .ok_or_else(|| bad("source is \"compose\" or \"manifests\"".into()))?;
    let path_in = body["path"]
        .as_str()
        .ok_or_else(|| bad("path is the file in the repository at main".into()))?;
    let path = clean_path("", path_in).map_err(bad)?;
    let name_arg = body["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let kind = body["kind"].as_str().unwrap_or("services");
    let dry_run = body["dry_run"].as_bool().unwrap_or(false);
    let text = String::from_utf8(read_file(app, project, &path)?)
        .map_err(|_| bad(format!("{path} is not UTF-8")))?;

    let (conversion, extra) = convert_source(source, kind, name_arg, body, &path, &text)?;
    let name = conversion.topology.name.clone();
    let target = format!("{DIR}/{name}.toml");
    let toml = spec::to_toml(&conversion.topology).map_err(bad)?;
    spec::parse(&toml).map_err(|e| bad(format!("the topology does not read back: {e}")))?;
    let replaces = read_file(app, project, &target).is_ok();
    let mut files = vec![(target.clone(), toml.clone().into_bytes())];
    files.extend(extra);
    let mut out = json!({
        "source": source,
        "name": name,
        "path": target,
        "files": files.iter().map(|(file, _)| file.clone()).collect::<Vec<String>>(),
        "replaces": replaces,
        "dry_run": dry_run,
        "toml": toml,
        "topology": conversion.topology,
        "report": conversion.report,
        "landed": Value::Null,
    });
    if !dry_run {
        let title = format!("Import the {name} topology from {path_in}");
        out["landed"] = land(app, project, &title, &files)?;
    }
    Ok(out)
}

/// Lands `files` on main of the project's repository as one owner's change, under `title`.
fn land(
    app: &App,
    project: &str,
    title: &str,
    files: &[(String, Vec<u8>)],
) -> Result<Value, (u16, String)> {
    if !app.weft.running() {
        return Err((503, "Weft is not running".into()));
    }
    let repo = crate::weft_hub::ensure_repo(app, project).map_err(|e| (503, e))?;
    let client = app
        .weft
        .client("owner")
        .map_err(|e| (503, format!("Weft: {e}")))?;
    let mut entries = Vec::with_capacity(files.len());
    for (file, bytes) in files {
        let hash = weft::hash::Hash::of(bytes);
        client
            .upload(&[hash], &|_| Ok(bytes.clone()))
            .map_err(|e| (503, format!("Weft: {e}")))?;
        entries.push(json!({ "path": file, "hash": hash.hex() }));
    }
    let edit = json!({ "title": title, "ref": "main", "files": entries });
    match client.post(&format!("/repos/{repo}/edits"), &edit) {
        Ok(landed) => {
            app.bus.publish(
                "weft_land",
                &json!({ "repo": repo, "change": landed["change"], "kind": "edit" }),
            );
            Ok(landed)
        }
        Err(e) if e.contains("409") || e.contains("clash") => Err((409, e)),
        Err(e) => Err((503, format!("Weft: {e}"))),
    }
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

    #[test]
    fn a_compose_file_converts_to_one_services_topology_named_by_the_request_or_the_file() {
        let text = "name: shop\nservices:\n  api:\n    image: 'x:1'\n";
        let (conversion, files) = convert_source(
            "compose",
            "services",
            None,
            &json!({}),
            "docker-compose.yml",
            text,
        )
        .unwrap();
        assert_eq!(conversion.topology.name, "shop");
        assert!(files.is_empty());
        let (named, _) = convert_source(
            "compose",
            "services",
            Some("web".into()),
            &json!({}),
            "docker-compose.yml",
            text,
        )
        .unwrap();
        assert_eq!(named.topology.name, "web");
        let k3s = convert_source(
            "compose",
            "k3s",
            None,
            &json!({}),
            "docker-compose.yml",
            text,
        );
        assert_eq!(k3s.unwrap_err().0, 400);
    }

    #[test]
    fn a_k3s_import_keeps_its_manifest_beside_the_topology_with_a_pinned_release() {
        let text = "kind: Deployment\nmetadata: {name: api}\nspec:\n  template:\n    spec:\n      containers:\n      - {name: api, image: 'redis:7'}\n";
        let body = json!({"k3s_version": "v1.31.4+k3s1"});
        let (conversion, files) = convert_source(
            "manifests",
            "k3s",
            Some("cluster".into()),
            &body,
            "deploy/app.yaml",
            text,
        )
        .unwrap();
        assert_eq!(conversion.topology.kind, Kind::K3s);
        let k3s = conversion.topology.k3s.unwrap();
        assert_eq!(
            k3s.manifests,
            [".plaza/topologies/cluster/manifests/app.yaml"]
        );
        assert_eq!(k3s.images, ["redis:7"]);
        assert_eq!(files[0].0, ".plaza/topologies/cluster/manifests/app.yaml");
        let no_version = convert_source(
            "manifests",
            "k3s",
            Some("cluster".into()),
            &json!({}),
            "deploy/app.yaml",
            text,
        );
        assert!(no_version.unwrap_err().1.contains("k3s_version"));
        let latest = json!({"k3s_version": "latest"});
        let bad = convert_source(
            "manifests",
            "k3s",
            Some("cluster".into()),
            &latest,
            "deploy/app.yaml",
            text,
        );
        assert!(bad.unwrap_err().1.contains("not a k3s release"));
        let unnamed = convert_source("manifests", "services", None, &json!({}), "app.yaml", text);
        assert!(unnamed.unwrap_err().1.contains("name"));
    }
}
