//! The owner's API for the prompt library (docs/design/prompt-library.md, "API"). Every `/v1` path is behind
//! `hub_auth::gate`, so only the owner reaches these handlers; the actor written to the history is `owner`.
//!
//! Each route closure parses the request and calls a plain handler that takes `&Prompts` and typed bodies, so the
//! tests call the handlers without an `App`.

use apz_http::{Request, Response, Router};
use apz_prompt::{Finding, Kind, Report, Stats, RULES_VERSION};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::events::Bus;
use crate::prompts::{Meta, NewPrompt, Prompt, Prompts, StoreError, Version, VersionMeta};
use crate::state::App;

const OWNER: &str = "owner";
const HISTORY_LIMIT: u32 = 500;

pub fn routes(router: Router<App>) -> Router<App> {
    // `check` and `rules` are registered before the `{id}` routes.
    router
        .post("/v1/prompts/check", |_, req, _| with_body(req, check))
        .get("/v1/prompts/rules", |_, _, _| Response::json(200, &rules_body()))
        .get("/v1/prompts", |app, req, _| {
            reply(list(&app.prompts, req.query_param("kind"), req.query_param("archived") == Some("true")))
        })
        .post("/v1/prompts", |app, req, _| with_body(req, |b: CreateBody| create(&app.prompts, &app.bus, b)))
        .get("/v1/prompts/{id}", |app, _, p| reply(detail(&app.prompts, p.get("id"))))
        .route("PUT", "/v1/prompts/{id}", |app, req, p| {
            with_body(req, |b: MetaBody| set_meta(&app.prompts, &app.bus, p.get("id"), b))
        })
        .get("/v1/prompts/{id}/versions/{v}", |app, _, p| reply(version(&app.prompts, p.get("id"), p.get("v"))))
        .get("/v1/prompts/{id}/versions/{v}/raw", |app, _, p| reply(raw(&app.prompts, p.get("id"), p.get("v"))))
        .post("/v1/prompts/{id}/versions", |app, req, p| {
            with_body(req, |b: VersionBody| add_version(&app.prompts, &app.bus, p.get("id"), b))
        })
        .post("/v1/prompts/{id}/publish", |app, req, p| {
            with_body(req, |b: PublishBody| publish(&app.prompts, &app.bus, p.get("id"), b))
        })
        .post("/v1/prompts/{id}/unpublish", |app, _, p| reply(unpublish(&app.prompts, &app.bus, p.get("id"))))
        .post("/v1/prompts/{id}/archive", |app, _, p| reply(archive(&app.prompts, &app.bus, p.get("id"), true)))
        .post("/v1/prompts/{id}/restore", |app, _, p| reply(archive(&app.prompts, &app.bus, p.get("id"), false)))
        .get("/v1/prompts/{id}/diff", |app, req, p| {
            reply(diff_versions(&app.prompts, p.get("id"), req.query_param("from"), req.query_param("to")))
        })
        .get("/v1/prompts/{id}/history", |app, _, p| reply(history(&app.prompts, p.get("id"))))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    id: Option<String>,
    kind: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    binding: Option<String>,
    body: String,
    #[serde(default)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct MetaBody {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    binding: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VersionBody {
    body: String,
    #[serde(default)]
    note: String,
    base_version: u32,
}

#[derive(Debug, Deserialize)]
struct PublishBody {
    version: u32,
}

#[derive(Debug, Deserialize)]
struct CheckBody {
    kind: String,
    body: String,
}

/// A version as the web client reads it: the stored record, its body and findings, a fresh check's stats, and, when
/// it was scored under older rules, today's report as `recheck`.
#[derive(Serialize)]
struct VersionView<'a> {
    #[serde(flatten)]
    meta: &'a VersionMeta,
    body: &'a str,
    findings: &'a [Finding],
    stats: Stats,
    stale: bool,
    recheck: Option<Report>,
}

fn view(kind: Kind, v: &Version) -> VersionView<'_> {
    let fresh = apz_prompt::check(kind, &v.body);
    let stale = v.meta.rules_version < RULES_VERSION;
    VersionView {
        meta: &v.meta,
        body: &v.body,
        findings: &v.findings,
        stats: fresh.stats.clone(),
        stale,
        recheck: stale.then_some(fresh),
    }
}

/// Maps a store error to its status: 404, 409, 422 or 500.
fn reply(result: Result<Response, StoreError>) -> Response {
    match result {
        Ok(response) => response,
        Err(StoreError::NotFound) => Response::error(404, "no such prompt or version"),
        Err(StoreError::Conflict(msg)) => Response::error(409, &msg),
        Err(StoreError::Invalid(msg)) => Response::error(422, &msg),
        Err(StoreError::Failed(msg)) => Response::error(500, &msg),
    }
}

/// Parses the body as `T` (400 when it is not valid JSON for the route), then runs the handler.
fn with_body<T: DeserializeOwned>(req: &Request, handle: impl FnOnce(T) -> Result<Response, StoreError>) -> Response {
    match req.json::<T>() {
        Ok(body) => reply(handle(body)),
        Err(_) => Response::error(400, "the body is not valid JSON for this route"),
    }
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

fn parse_kind(raw: &str) -> Result<Kind, StoreError> {
    Kind::parse(raw).ok_or_else(|| StoreError::Invalid(format!("kind is system, agents_md or fragment, not {raw}")))
}

fn number(raw: &str) -> Option<u32> {
    raw.parse().ok()
}

fn find(prompts: &Prompts, id: &str) -> Result<Prompt, StoreError> {
    prompts.store().get(id)?.ok_or(StoreError::NotFound)
}

fn load_version(prompts: &Prompts, id: &str, v: u32) -> Result<Version, StoreError> {
    prompts.store().version(id, v)?.ok_or(StoreError::NotFound)
}

/// A prompt as the list and detail routes show it: its fields, the live version's score, and the best version.
fn summary(prompts: &Prompts, p: &Prompt) -> Result<Value, StoreError> {
    let versions = prompts.store().versions(&p.id)?;
    let best = apz_prompt::best_version(versions.iter().map(|v| (v.version, v.score)));
    let score = p.active_version.and_then(|live| versions.iter().find(|v| v.version == live)).map(|v| v.score);
    Ok(json!({
        "id": p.id,
        "kind": p.kind.as_str(),
        "title": p.title,
        "description": p.description,
        "tags": p.tags,
        "binding": p.binding,
        "active_version": p.active_version,
        "latest_version": p.latest_version,
        "best_version": best,
        "score": score,
        "created_ms": p.created_ms,
        "updated_ms": p.updated_ms,
        "archived_ms": p.archived_ms,
    }))
}

fn prompt_reply(prompts: &Prompts, prompt: Prompt, status: u16) -> Result<Response, StoreError> {
    let head = summary(prompts, &prompt)?;
    Ok(Response::json(status, &json!({ "prompt": head })))
}

fn list(prompts: &Prompts, kind: Option<&str>, archived: bool) -> Result<Response, StoreError> {
    let kind = kind.map(parse_kind).transpose()?;
    let mut out = Vec::new();
    for p in prompts.store().list(archived)? {
        if kind.is_some_and(|k| k != p.kind) {
            continue;
        }
        out.push(summary(prompts, &p)?);
    }
    Ok(Response::json(200, &json!({ "prompts": out })))
}

/// Publishes the change on the event bus: a `prompt` event with the prompt's id and the action taken.
fn announce(bus: &Bus, id: &str, action: &str) {
    bus.publish("prompt", &json!({ "id": id, "action": action }));
}

fn create(prompts: &Prompts, bus: &Bus, b: CreateBody) -> Result<Response, StoreError> {
    let kind = parse_kind(&b.kind)?;
    let id = match b.id {
        Some(id) => id,
        None => apz_prompt::slug(&b.title),
    };
    let new = NewPrompt { id, kind, title: b.title, description: b.description, tags: b.tags, binding: b.binding };
    let prompt = prompts.create(&new, b.body, b.note, OWNER, now())?;
    announce(bus, &prompt.id, "create");
    prompt_reply(prompts, prompt, 201)
}

fn set_meta(prompts: &Prompts, bus: &Bus, id: &str, b: MetaBody) -> Result<Response, StoreError> {
    let meta = Meta { title: b.title, description: b.description, tags: b.tags, binding: b.binding };
    let prompt = prompts.set_meta(id, &meta, OWNER, now())?;
    announce(bus, id, "update");
    prompt_reply(prompts, prompt, 200)
}

fn detail(prompts: &Prompts, id: &str) -> Result<Response, StoreError> {
    let prompt = find(prompts, id)?;
    let head = summary(prompts, &prompt)?;
    let versions = prompts.store().versions(id)?;
    let live = match prompt.active_version {
        Some(v) => Some(load_version(prompts, id, v)?),
        None => None,
    };
    let live_view = live.as_ref().map(|v| view(prompt.kind, v));
    Ok(Response::json(200, &json!({ "prompt": head, "versions": versions, "live": live_view })))
}

fn version(prompts: &Prompts, id: &str, v: &str) -> Result<Response, StoreError> {
    let prompt = find(prompts, id)?;
    let ver = load_version(prompts, id, number(v).ok_or(StoreError::NotFound)?)?;
    Ok(Response::json(200, &view(prompt.kind, &ver)))
}

fn raw(prompts: &Prompts, id: &str, v: &str) -> Result<Response, StoreError> {
    let prompt = find(prompts, id)?;
    let ver = load_version(prompts, id, number(v).ok_or(StoreError::NotFound)?)?;
    let name = if prompt.kind == Kind::AgentsMd {
        "AGENTS.md".to_string()
    } else {
        format!("{}-v{}.md", prompt.id, ver.meta.version)
    };
    Ok(Response::new(200, "text/markdown; charset=utf-8", ver.body)
        .with_header("content-disposition", &format!("attachment; filename=\"{name}\"")))
}

fn add_version(prompts: &Prompts, bus: &Bus, id: &str, b: VersionBody) -> Result<Response, StoreError> {
    let (version, created) = prompts.add_version(id, b.base_version, b.body, b.note, OWNER, now())?;
    if created {
        announce(bus, id, "version");
    }
    let status = if created { 201 } else { 200 };
    Ok(Response::json(status, &json!({ "version": version, "created": created })))
}

fn publish(prompts: &Prompts, bus: &Bus, id: &str, b: PublishBody) -> Result<Response, StoreError> {
    let prompt = prompts.publish(id, b.version, OWNER, now())?;
    announce(bus, id, "publish");
    prompt_reply(prompts, prompt, 200)
}

fn unpublish(prompts: &Prompts, bus: &Bus, id: &str) -> Result<Response, StoreError> {
    let prompt = prompts.unpublish(id, OWNER, now())?;
    announce(bus, id, "unpublish");
    prompt_reply(prompts, prompt, 200)
}

fn archive(prompts: &Prompts, bus: &Bus, id: &str, archived: bool) -> Result<Response, StoreError> {
    let prompt = prompts.set_archived(id, archived, OWNER, now())?;
    announce(bus, id, if archived { "archive" } else { "restore" });
    prompt_reply(prompts, prompt, 200)
}

fn diff_versions(prompts: &Prompts, id: &str, from: Option<&str>, to: Option<&str>) -> Result<Response, StoreError> {
    let (Some(from), Some(to)) = (from.and_then(number), to.and_then(number)) else {
        return Err(StoreError::Invalid("from and to must be version numbers".into()));
    };
    let prompt = find(prompts, id)?;
    let old = load_version(prompts, id, from)?;
    let new = load_version(prompts, id, to)?;
    let lines = apz_prompt::diff(&old.body, &new.body);
    Ok(Response::json(200, &json!({ "from": view(prompt.kind, &old), "to": view(prompt.kind, &new), "lines": lines })))
}

fn history(prompts: &Prompts, id: &str) -> Result<Response, StoreError> {
    find(prompts, id)?;
    let events = prompts.store().events(id, HISTORY_LIMIT)?;
    Ok(Response::json(200, &json!({ "events": events })))
}

fn check(b: CheckBody) -> Result<Response, StoreError> {
    let kind = parse_kind(&b.kind)?;
    Ok(Response::json(200, &apz_prompt::check(kind, &b.body)))
}

fn rules_body() -> Value {
    json!({
        "rules_version": RULES_VERSION,
        "rules": apz_prompt::rules(),
        "variables": apz_prompt::VARIABLES,
        "templates": {
            "system": apz_prompt::template(Kind::System),
            "agents_md": apz_prompt::template(Kind::AgentsMd),
            "fragment": apz_prompt::template(Kind::Fragment),
        },
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::prompts::store_mem::MemoryPrompts;

    fn library() -> Prompts {
        Prompts::new(Arc::new(MemoryPrompts::in_memory()))
    }

    fn json_of(r: &Response) -> Value {
        serde_json::from_slice(r.bytes()).expect("a JSON body")
    }

    fn new_prompt(title: &str, body: String) -> CreateBody {
        CreateBody {
            id: None,
            kind: "system".into(),
            title: title.into(),
            description: String::new(),
            tags: vec![],
            binding: None,
            body,
            note: String::new(),
        }
    }

    #[test]
    fn owner_round_trip() {
        let lib = library();
        let bus = Bus::default();
        let base = apz_prompt::template(Kind::System).to_string();

        let created = create(&lib, &bus, new_prompt("Planner", base.clone())).expect("create");
        assert_eq!(created.status, 201);
        assert_eq!(json_of(&created)["prompt"]["id"], "planner");

        let got = detail(&lib, "planner").expect("detail");
        assert_eq!(got.status, 200);
        assert_eq!(json_of(&got)["prompt"]["latest_version"].as_u64(), Some(1));
        assert_eq!(json_of(&got)["versions"].as_array().map(Vec::len), Some(1));

        let v2_body = format!("{base}\n## Notes\n\nKeep answers short.\n");
        let added = add_version(
            &lib,
            &bus,
            "planner",
            VersionBody { body: v2_body.clone(), note: "shorter".into(), base_version: 1 },
        )
        .expect("add version");
        assert_eq!(added.status, 201);
        let unchanged = add_version(
            &lib,
            &bus,
            "planner",
            VersionBody { body: v2_body.clone(), note: String::new(), base_version: 2 },
        )
        .expect("unchanged body");
        assert_eq!(unchanged.status, 200);
        assert!(matches!(
            add_version(&lib, &bus, "planner", VersionBody { body: "other".into(), note: String::new(), base_version: 1 }),
            Err(StoreError::Conflict(_))
        ));

        let diff = diff_versions(&lib, "planner", Some("1"), Some("2")).expect("diff");
        assert_eq!(diff.status, 200);
        assert!(json_of(&diff)["lines"].as_array().is_some_and(|l| !l.is_empty()));
        assert!(matches!(diff_versions(&lib, "planner", Some("1"), None), Err(StoreError::Invalid(_))));

        let live = publish(&lib, &bus, "planner", PublishBody { version: 2 }).expect("publish");
        assert_eq!(live.status, 200);
        assert_eq!(json_of(&live)["prompt"]["active_version"].as_u64(), Some(2));
        assert!(matches!(publish(&lib, &bus, "planner", PublishBody { version: 99 }), Err(StoreError::Invalid(_))));

        let events = history(&lib, "planner").expect("history");
        assert!(json_of(&events)["events"].as_array().is_some_and(|e| e.len() >= 4));

        let text = raw(&lib, "planner", "2").expect("raw");
        assert_eq!(text.status, 200);
        assert_eq!(text.bytes(), v2_body.as_bytes());

        let report = check(CheckBody { kind: "agents_md".into(), body: "# Hi\n".into() }).expect("check");
        assert_eq!(report.status, 200);
        assert!(matches!(check(CheckBody { kind: "nope".into(), body: String::new() }), Err(StoreError::Invalid(_))));

        let rules = Response::json(200, &rules_body());
        assert_eq!(json_of(&rules)["rules_version"].as_u64(), Some(u64::from(RULES_VERSION)));
    }

    #[test]
    fn missing_and_duplicate_cases() {
        let lib = library();
        let bus = Bus::default();
        let base = apz_prompt::template(Kind::System).to_string();
        create(&lib, &bus, new_prompt("Planner", base.clone())).expect("create");

        assert!(matches!(create(&lib, &bus, new_prompt("Planner", base)), Err(StoreError::Conflict(_))));
        assert!(matches!(detail(&lib, "missing"), Err(StoreError::NotFound)));
        assert!(matches!(version(&lib, "planner", "9"), Err(StoreError::NotFound)));
        assert!(matches!(history(&lib, "missing"), Err(StoreError::NotFound)));
    }

    #[test]
    fn changes_publish_prompt_events() {
        let lib = library();
        let bus = Bus::default();
        let events = bus.subscribe();
        let base = apz_prompt::template(Kind::System).to_string();

        create(&lib, &bus, new_prompt("Planner", base.clone())).expect("create");
        let v2 = format!("{base}\n## Notes\n\nKeep answers short.\n");
        add_version(&lib, &bus, "planner", VersionBody { body: v2.clone(), note: "shorter".into(), base_version: 1 }).expect("version");
        // The same body again is not a new version, so it publishes nothing.
        add_version(&lib, &bus, "planner", VersionBody { body: v2, note: String::new(), base_version: 2 }).expect("unchanged");
        publish(&lib, &bus, "planner", PublishBody { version: 2 }).expect("publish");
        // A refused publish changes nothing, so it publishes nothing.
        assert!(publish(&lib, &bus, "planner", PublishBody { version: 99 }).is_err());
        let meta = MetaBody { title: "Planner v2".into(), description: String::new(), tags: vec![], binding: Some("role:planner".into()) };
        set_meta(&lib, &bus, "planner", meta).expect("meta");
        unpublish(&lib, &bus, "planner").expect("unpublish");
        archive(&lib, &bus, "planner", true).expect("archive");
        archive(&lib, &bus, "planner", false).expect("restore");

        let got: Vec<(String, String, String)> = events
            .try_iter()
            .map(|f| {
                let data: Value = serde_json::from_str(&f.data).expect("JSON data");
                (f.kind, data["id"].as_str().unwrap_or_default().to_string(), data["action"].as_str().unwrap_or_default().to_string())
            })
            .collect();
        let actions: Vec<&str> = got.iter().map(|(_, _, action)| action.as_str()).collect();
        assert_eq!(actions, ["create", "version", "publish", "update", "unpublish", "archive", "restore"]);
        assert!(got.iter().all(|(kind, id, _)| kind == "prompt" && id == "planner"));
    }
}
