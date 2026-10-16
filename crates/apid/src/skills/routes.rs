//! The owner's API for the skill library (docs/design/skill-library.md, "API"). Every `/v1` path is behind
//! `hub_auth::gate`, so only the owner reaches these handlers; the actor written to the history is `owner`.
//!
//! Each route closure parses the request and calls a plain handler that takes `&Skills` and typed bodies, so the
//! tests call the handlers without an `App`.

use apz_http::{Request, Response, RouteDoc, Router};
use apz_skill::{FileInfo, RULES_VERSION, Report};
use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    FileBody, FileIn, MAX_TAG, MAX_TAGS, MAX_TITLE, NewSkill, Skill, SkillFile, Skills, Source,
    StoreError, Version, VersionMeta, sha256_hex, valid_binding, valid_id,
};
use crate::events::Bus;
use crate::state::App;

const OWNER: &str = "owner";
const HISTORY_LIMIT: u32 = 500;

pub fn routes(router: Router<App>) -> Router<App> {
    // `check` and `rules` are registered before the `{id}` routes.
    router
        .post("/v1/skills/check", |_, req, _| with_body(req, check))
        .doc(RouteDoc::new("Check a skill's SKILL.md and files against the rules").errors(&[400]))
        .get("/v1/skills/rules", |_, _, _| {
            Response::json(200, &rules_body())
        })
        .doc(RouteDoc::new("List the skill rules and the template"))
        .get("/v1/skills", |app, req, _| {
            reply(list(
                &app.skills,
                req.query_param("archived") == Some("true"),
            ))
        })
        .doc(RouteDoc::new("List the skills, or the archived ones"))
        .post("/v1/skills", |app, req, _| {
            with_body(req, |b: CreateBody| create(&app.skills, &app.bus, b))
        })
        .doc(
            RouteDoc::new("Create a skill")
                .status(201)
                .errors(&[400, 409, 422]),
        )
        .get("/v1/skills/{id}", |app, _, p| {
            reply(detail(&app.skills, p.get("id")))
        })
        .doc(RouteDoc::new("Get a skill with its versions").errors(&[404]))
        .route("PUT", "/v1/skills/{id}", |app, req, p| {
            with_body(req, |b: MetaBody| {
                set_meta(&app.skills, &app.bus, p.get("id"), b)
            })
        })
        .doc(RouteDoc::new("Update a skill's title and tags").errors(&[400, 422]))
        .route("PUT", "/v1/skills/{id}/bindings", |app, req, p| {
            with_body(req, |b: BindingsBody| {
                set_bindings(&app.skills, &app.bus, p.get("id"), b)
            })
        })
        .doc(RouteDoc::new("Set the consumers a skill is bound to").errors(&[400, 422]))
        .get("/v1/skills/{id}/versions/{v}", |app, _, p| {
            reply(version(&app.skills, p.get("id"), p.get("v")))
        })
        .doc(RouteDoc::new("Get one version of a skill").errors(&[404]))
        // `{path..}` takes the rest of the path, so a file in a folder keeps its slashes.
        .get(
            "/v1/skills/{id}/versions/{v}/files/{path..}",
            |app, _, p| reply(file(&app.skills, p.get("id"), p.get("v"), p.get("path"))),
        )
        .doc(RouteDoc::new("Download one file of a skill version").errors(&[404]))
        .post("/v1/skills/{id}/versions", |app, req, p| {
            with_body(req, |b: VersionBody| {
                add_version(&app.skills, &app.bus, p.get("id"), b)
            })
        })
        .doc(
            RouteDoc::new("Add a version of a skill")
                .status(201)
                .errors(&[400, 404, 409, 422]),
        )
        .post("/v1/skills/{id}/publish", |app, req, p| {
            with_body(req, |b: PublishBody| {
                publish(&app.skills, &app.bus, p.get("id"), b)
            })
        })
        .doc(RouteDoc::new("Publish a version as the live one").errors(&[400, 404, 422]))
        .post("/v1/skills/{id}/unpublish", |app, _, p| {
            reply(unpublish(&app.skills, &app.bus, p.get("id")))
        })
        .doc(RouteDoc::new("Unpublish a skill"))
        .post("/v1/skills/{id}/archive", |app, _, p| {
            reply(archive(&app.skills, &app.bus, p.get("id"), true))
        })
        .doc(RouteDoc::new("Archive a skill"))
        .post("/v1/skills/{id}/restore", |app, _, p| {
            reply(archive(&app.skills, &app.bus, p.get("id"), false))
        })
        .doc(RouteDoc::new("Restore an archived skill"))
        .get("/v1/skills/{id}/diff", |app, req, p| {
            reply(diff_versions(
                &app.skills,
                p.get("id"),
                req.query_param("from"),
                req.query_param("to"),
            ))
        })
        .doc(RouteDoc::new("Compare two versions of a skill").errors(&[404, 422]))
        .get("/v1/skills/{id}/history", |app, _, p| {
            reply(history(&app.skills, p.get("id")))
        })
        .doc(RouteDoc::new("List a skill's history").errors(&[404]))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    id: Option<String>,
    title: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    bindings: Vec<String>,
    skill_md: String,
    #[serde(default)]
    files: Vec<FileBodyIn>,
    #[serde(default)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct MetaBody {
    title: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BindingsBody {
    #[serde(default)]
    bindings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct VersionBody {
    skill_md: String,
    #[serde(default)]
    files: Vec<FileBodyIn>,
    #[serde(default)]
    note: String,
    base_version: u32,
}

#[derive(Debug, Deserialize)]
struct PublishBody {
    version: u32,
}

/// A file to save: `content_b64` (new bytes) or `sha256` (an unchanged file already stored), never both.
#[derive(Debug, Deserialize)]
struct FileBodyIn {
    path: String,
    #[serde(default)]
    executable: bool,
    content_b64: Option<String>,
    sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CheckBody {
    skill_md: String,
    #[serde(default)]
    files: Vec<CheckFile>,
}

#[derive(Debug, Deserialize)]
struct CheckFile {
    path: String,
    size: u64,
    #[serde(default)]
    executable: bool,
    text: Option<String>,
}

/// A version as the web client reads it: the stored record, its SKILL.md, files and findings, and, when it was
/// scored under older rules, today's report as `recheck`.
#[derive(Serialize)]
struct VersionView<'a> {
    #[serde(flatten)]
    meta: &'a VersionMeta,
    skill_md: &'a str,
    files: &'a [SkillFile],
    findings: &'a [apz_skill::Finding],
    stale: bool,
    recheck: Option<Report>,
}

fn view<'a>(skills: &Skills, v: &'a Version) -> Result<VersionView<'a>, StoreError> {
    let stale = v.meta.rules_version < RULES_VERSION;
    let recheck = if stale {
        let infos = skills.file_infos(&v.files)?;
        Some(apz_skill::check(&v.skill_md, &infos))
    } else {
        None
    };
    Ok(VersionView {
        meta: &v.meta,
        skill_md: &v.skill_md,
        files: &v.files,
        findings: &v.findings,
        stale,
        recheck,
    })
}

/// The files a diff lists: added, removed, and changed (sha256 or the executable bit).
#[derive(Serialize)]
struct FileChanges {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
}

fn file_changes(old: &[SkillFile], new: &[SkillFile]) -> FileChanges {
    let mut out = FileChanges {
        added: vec![],
        removed: vec![],
        changed: vec![],
    };
    for f in new {
        match old.iter().find(|o| o.path == f.path) {
            None => out.added.push(f.path.clone()),
            Some(o) if o.sha256 != f.sha256 || o.executable != f.executable => {
                out.changed.push(f.path.clone())
            }
            Some(_) => {}
        }
    }
    for f in old {
        if !new.iter().any(|n| n.path == f.path) {
            out.removed.push(f.path.clone());
        }
    }
    out
}

/// Maps a store error to its status: 404, 409, 422 or 500.
fn reply(result: Result<Response, StoreError>) -> Response {
    match result {
        Ok(response) => response,
        Err(StoreError::NotFound) => Response::error(404, "no such skill or version"),
        Err(StoreError::Conflict(msg)) => Response::error(409, &msg),
        Err(StoreError::Invalid(msg)) => Response::error(422, &msg),
        Err(StoreError::Failed(msg)) => Response::error(500, &msg),
    }
}

/// Parses the body as `T` (400 when it is not valid JSON for the route), then runs the handler.
fn with_body<T: DeserializeOwned>(
    req: &Request,
    handle: impl FnOnce(T) -> Result<Response, StoreError>,
) -> Response {
    match req.json::<T>() {
        Ok(body) => reply(handle(body)),
        Err(_) => Response::error(400, "the body is not valid JSON for this route"),
    }
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::Invalid(msg.into())
}

/// Title and tags, under the same limits as the prompt library.
fn validate_meta(title: &str, tags: &[String]) -> Result<(), StoreError> {
    let title_len = title.trim().chars().count();
    if title_len == 0 || title_len > MAX_TITLE {
        return Err(invalid(format!("title must be 1-{MAX_TITLE} characters")));
    }
    if tags.len() > MAX_TAGS
        || tags
            .iter()
            .any(|t| t.trim().is_empty() || t.chars().count() > MAX_TAG)
    {
        return Err(invalid(format!(
            "at most {MAX_TAGS} tags of 1-{MAX_TAG} characters"
        )));
    }
    Ok(())
}

/// Checks each binding (`plaza` or `role:<role>`) and returns them sorted and without repeats, as the store keeps them.
fn bindings_checked(mut bindings: Vec<String>) -> Result<Vec<String>, StoreError> {
    if let Some(b) = bindings.iter().find(|b| !valid_binding(b)) {
        return Err(invalid(format!(
            "{b} is not a binding (plaza or role:<role>)"
        )));
    }
    bindings.sort();
    bindings.dedup();
    Ok(bindings)
}

/// The files as `Skills::prepare` takes them: base64 bytes, or the sha256 of a file already stored.
fn file_ins(files: Vec<FileBodyIn>) -> Result<Vec<FileIn>, StoreError> {
    files
        .into_iter()
        .map(|f| {
            let body = match (f.content_b64, f.sha256) {
                (Some(b64), None) => FileBody::Bytes(
                    base64::engine::general_purpose::STANDARD
                        .decode(b64.as_bytes())
                        .map_err(|_| invalid(format!("{}: content_b64 is not base64", f.path)))?,
                ),
                (None, Some(sha)) => FileBody::Known(sha),
                _ => {
                    return Err(invalid(format!(
                        "{}: give content_b64 or sha256, not both",
                        f.path
                    )));
                }
            };
            Ok(FileIn {
                path: f.path,
                body,
                executable: f.executable,
            })
        })
        .collect()
}

/// True when SKILL.md and the files (path, sha256 and executable bit) are the latest version's.
fn unchanged(latest: &Version, skill_md: &str, files: &[FileIn]) -> bool {
    if sha256_hex(skill_md.as_bytes()) != latest.meta.sha256 || files.len() != latest.files.len() {
        return false;
    }
    files.iter().all(|f| {
        let sha = match &f.body {
            FileBody::Bytes(bytes) => sha256_hex(bytes),
            FileBody::Known(sha) => sha.clone(),
        };
        latest
            .files
            .iter()
            .any(|old| old.path == f.path && old.sha256 == sha && old.executable == f.executable)
    })
}

/// The Content-Type of a file by its extension. Text the extension does not name is text/plain (never HTML), and
/// anything else is served as bytes.
fn content_type(path: &str, bytes: &[u8]) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "md" => "text/markdown; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ if std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0) => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

fn number(raw: &str) -> Result<u32, StoreError> {
    raw.parse().map_err(|_| StoreError::NotFound)
}

fn find(skills: &Skills, id: &str) -> Result<Skill, StoreError> {
    skills.store().get(id)?.ok_or(StoreError::NotFound)
}

fn load_version(skills: &Skills, id: &str, v: u32) -> Result<Version, StoreError> {
    skills.store().version(id, v)?.ok_or(StoreError::NotFound)
}

/// A skill as the list and detail routes show it: its fields, the live version's score, and the best version.
fn summary(skills: &Skills, s: &Skill) -> Result<Value, StoreError> {
    let versions = skills.store().versions(&s.id)?;
    let best = apz_prompt::best_version(versions.iter().map(|v| (v.version, v.score)));
    let score = s
        .active_version
        .and_then(|live| versions.iter().find(|v| v.version == live))
        .map(|v| v.score);
    Ok(json!({
        "id": s.id,
        "name": s.name,
        "title": s.title,
        "description": s.description,
        "tags": s.tags,
        "bindings": s.bindings,
        "active_version": s.active_version,
        "latest_version": s.latest_version,
        "best_version": best,
        "score": score,
        "source": s.source,
        "created_ms": s.created_ms,
        "updated_ms": s.updated_ms,
        "archived_ms": s.archived_ms,
    }))
}

fn skill_reply(skills: &Skills, skill: Skill, status: u16) -> Result<Response, StoreError> {
    let head = summary(skills, &skill)?;
    Ok(Response::json(status, &json!({ "skill": head })))
}

fn list(skills: &Skills, archived: bool) -> Result<Response, StoreError> {
    let mut out = Vec::new();
    for s in skills.store().list(archived)? {
        out.push(summary(skills, &s)?);
    }
    Ok(Response::json(200, &json!({ "skills": out })))
}

/// Publishes the change on the event bus: a `skill` event with the skill's id and the action taken.
fn announce(bus: &Bus, id: &str, action: &str) {
    bus.publish("skill", &json!({ "id": id, "action": action }));
}

fn create(skills: &Skills, bus: &Bus, b: CreateBody) -> Result<Response, StoreError> {
    let id = b.id.unwrap_or_else(|| apz_prompt::slug(&b.title));
    if !valid_id(&id) {
        return Err(invalid(
            "id must be 1-63 lower-case letters, digits and dashes, starting with a letter or digit",
        ));
    }
    validate_meta(&b.title, &b.tags)?;
    let bindings = bindings_checked(b.bindings)?;
    let files = file_ins(b.files)?;
    let first = skills.prepare(b.skill_md, files, b.note, None)?;
    let new = NewSkill {
        id,
        title: b.title,
        tags: b.tags,
        bindings,
        source: Source::Built,
    };
    let skill = skills.store().create(&new, &first, OWNER, now())?;
    announce(bus, &skill.id, "create");
    skill_reply(skills, skill, 201)
}

fn set_meta(skills: &Skills, bus: &Bus, id: &str, b: MetaBody) -> Result<Response, StoreError> {
    validate_meta(&b.title, &b.tags)?;
    let skill = skills
        .store()
        .set_meta(id, &b.title, &b.tags, OWNER, now())?;
    announce(bus, id, "update");
    skill_reply(skills, skill, 200)
}

fn set_bindings(
    skills: &Skills,
    bus: &Bus,
    id: &str,
    b: BindingsBody,
) -> Result<Response, StoreError> {
    let bindings = bindings_checked(b.bindings)?;
    let skill = skills.store().set_bindings(id, &bindings, OWNER, now())?;
    announce(bus, id, "bindings");
    skill_reply(skills, skill, 200)
}

fn detail(skills: &Skills, id: &str) -> Result<Response, StoreError> {
    let skill = find(skills, id)?;
    let head = summary(skills, &skill)?;
    let versions = skills.store().versions(id)?;
    let live_version = match skill.active_version {
        Some(v) => Some(load_version(skills, id, v)?),
        None => None,
    };
    let live = live_version.as_ref().map(|v| view(skills, v)).transpose()?;
    Ok(Response::json(
        200,
        &json!({ "skill": head, "versions": versions, "live": live }),
    ))
}

fn version(skills: &Skills, id: &str, v: &str) -> Result<Response, StoreError> {
    find(skills, id)?;
    let ver = load_version(skills, id, number(v)?)?;
    Ok(Response::json(200, &view(skills, &ver)?))
}

/// One file's bytes, with its Content-Type by extension.
fn file(skills: &Skills, id: &str, v: &str, path: &str) -> Result<Response, StoreError> {
    find(skills, id)?;
    let ver = load_version(skills, id, number(v)?)?;
    let entry = ver
        .files
        .iter()
        .find(|f| f.path == path)
        .ok_or(StoreError::NotFound)?;
    let bytes = skills
        .blobs()
        .get(&entry.sha256)
        .map_err(StoreError::Failed)?;
    let ct = content_type(path, &bytes);
    Ok(Response::new(200, ct, bytes).with_header("x-content-type-options", "nosniff"))
}

/// Adds a version, or returns the latest one unchanged (`created` false) when SKILL.md and every file equal it.
fn add_version(
    skills: &Skills,
    bus: &Bus,
    id: &str,
    b: VersionBody,
) -> Result<Response, StoreError> {
    let files = file_ins(b.files)?;
    let latest_meta = skills
        .store()
        .versions(id)?
        .pop()
        .ok_or(StoreError::NotFound)?;
    let latest = load_version(skills, id, latest_meta.version)?;
    if unchanged(&latest, &b.skill_md, &files) {
        return Ok(Response::json(
            200,
            &json!({ "version": latest.meta, "created": false }),
        ));
    }
    let next = skills.prepare(b.skill_md, files, b.note, None)?;
    let meta = skills
        .store()
        .add_version(id, b.base_version, &next, OWNER, now())?;
    announce(bus, id, "version");
    Ok(Response::json(
        201,
        &json!({ "version": meta, "created": true }),
    ))
}

fn publish(skills: &Skills, bus: &Bus, id: &str, b: PublishBody) -> Result<Response, StoreError> {
    find(skills, id)?;
    let skill = skills.publish(id, b.version, OWNER, now())?;
    announce(bus, id, "publish");
    skill_reply(skills, skill, 200)
}

fn unpublish(skills: &Skills, bus: &Bus, id: &str) -> Result<Response, StoreError> {
    let skill = skills.store().set_active(id, None, OWNER, now())?;
    announce(bus, id, "unpublish");
    skill_reply(skills, skill, 200)
}

fn archive(skills: &Skills, bus: &Bus, id: &str, archived: bool) -> Result<Response, StoreError> {
    let skill = skills.store().set_archived(id, archived, OWNER, now())?;
    announce(bus, id, if archived { "archive" } else { "restore" });
    skill_reply(skills, skill, 200)
}

fn diff_versions(
    skills: &Skills,
    id: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Response, StoreError> {
    let (Some(from), Some(to)) = (
        from.and_then(|v| v.parse::<u32>().ok()),
        to.and_then(|v| v.parse::<u32>().ok()),
    ) else {
        return Err(invalid("from and to must be version numbers"));
    };
    find(skills, id)?;
    let old = load_version(skills, id, from)?;
    let new = load_version(skills, id, to)?;
    let lines = apz_prompt::diff(&old.skill_md, &new.skill_md);
    let files = file_changes(&old.files, &new.files);
    let old_view = view(skills, &old)?;
    let new_view = view(skills, &new)?;
    Ok(Response::json(
        200,
        &json!({ "from": old_view, "to": new_view, "skill_md": lines, "files": files }),
    ))
}

fn history(skills: &Skills, id: &str) -> Result<Response, StoreError> {
    find(skills, id)?;
    let events = skills.store().events(id, HISTORY_LIMIT)?;
    Ok(Response::json(200, &json!({ "events": events })))
}

fn check(b: CheckBody) -> Result<Response, StoreError> {
    let infos: Vec<FileInfo> = b
        .files
        .into_iter()
        .map(|f| FileInfo {
            path: f.path,
            size: f.size,
            executable: f.executable,
            text: f.text,
        })
        .collect();
    Ok(Response::json(200, &apz_skill::check(&b.skill_md, &infos)))
}

fn rules_body() -> Value {
    json!({
        "rules_version": RULES_VERSION,
        "rules": apz_skill::rules(),
        "template": apz_skill::template(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::skills::store_mem::MemorySkills;
    use crate::skills::tests::MemBlobs;

    const GOOD: &str = "---\nname: pdf-forms\ndescription: Fill PDF forms. Use when the task involves a PDF with form fields.\n---\n# PDF forms\n\nRead [the reference](reference.md) first.\n";
    const REFERENCE: &[u8] = b"# Reference\n";

    fn library() -> Skills {
        Skills::new(
            Arc::new(MemorySkills::in_memory()),
            Arc::new(MemBlobs::default()),
        )
    }

    fn json_of(r: &Response) -> Value {
        serde_json::from_slice(r.bytes()).expect("a JSON body")
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn in_file(path: &str, content_b64: Option<String>, sha256: Option<String>) -> FileBodyIn {
        FileBodyIn {
            path: path.into(),
            executable: false,
            content_b64,
            sha256,
        }
    }

    fn new_skill(md: &str) -> CreateBody {
        CreateBody {
            id: Some("pdf-forms".into()),
            title: "PDF forms".into(),
            tags: vec![],
            bindings: vec!["role:builder".into()],
            skill_md: md.into(),
            files: vec![in_file("reference.md", Some(b64(REFERENCE)), None)],
            note: String::new(),
        }
    }

    fn version_body(md: &str, files: Vec<FileBodyIn>, base: u32) -> VersionBody {
        VersionBody {
            skill_md: md.into(),
            files,
            note: "more".into(),
            base_version: base,
        }
    }

    #[test]
    fn owner_round_trip() {
        let lib = library();
        let bus = Bus::default();
        let created = create(&lib, &bus, new_skill(GOOD)).expect("create");
        assert_eq!(created.status, 201);
        assert_eq!(json_of(&created)["skill"]["id"], "pdf-forms");
        assert_eq!(
            json_of(&created)["skill"]["latest_version"].as_u64(),
            Some(1)
        );

        let got = detail(&lib, "pdf-forms").expect("detail");
        assert_eq!(json_of(&got)["versions"].as_array().map(Vec::len), Some(1));
        assert!(json_of(&got)["live"].is_null());

        // Version 2 changes SKILL.md; the reference file is sent by its sha256 alone.
        let more = format!("{GOOD}\nMore.\n");
        let added = add_version(
            &lib,
            &bus,
            "pdf-forms",
            version_body(
                &more,
                vec![in_file("reference.md", None, Some(sha256_hex(REFERENCE)))],
                1,
            ),
        )
        .expect("version 2");
        assert_eq!(added.status, 201);
        assert_eq!(json_of(&added)["version"]["version"].as_u64(), Some(2));

        // The same SKILL.md and files again is not a new version.
        let same = add_version(
            &lib,
            &bus,
            "pdf-forms",
            version_body(
                &more,
                vec![in_file("reference.md", None, Some(sha256_hex(REFERENCE)))],
                2,
            ),
        )
        .expect("unchanged");
        assert_eq!(same.status, 200);
        assert_eq!(json_of(&same)["created"], false);

        // A stale base is a conflict.
        assert!(matches!(
            add_version(
                &lib,
                &bus,
                "pdf-forms",
                version_body(
                    GOOD,
                    vec![in_file("reference.md", None, Some(sha256_hex(REFERENCE)))],
                    1
                )
            ),
            Err(StoreError::Conflict(_))
        ));

        let diff = diff_versions(&lib, "pdf-forms", Some("1"), Some("2")).expect("diff");
        assert_eq!(diff.status, 200);
        assert!(
            json_of(&diff)["skill_md"]
                .as_array()
                .is_some_and(|l| !l.is_empty())
        );
        assert!(
            json_of(&diff)["files"]["changed"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );
        assert!(matches!(
            diff_versions(&lib, "pdf-forms", Some("1"), None),
            Err(StoreError::Invalid(_))
        ));

        let live = publish(&lib, &bus, "pdf-forms", PublishBody { version: 2 }).expect("publish");
        assert_eq!(json_of(&live)["skill"]["active_version"].as_u64(), Some(2));
        assert!(matches!(
            publish(&lib, &bus, "pdf-forms", PublishBody { version: 9 }),
            Err(StoreError::Invalid(_))
        ));
        let live_detail = detail(&lib, "pdf-forms").expect("detail");
        assert_eq!(json_of(&live_detail)["live"]["version"].as_u64(), Some(2));

        let events = history(&lib, "pdf-forms").expect("history");
        assert!(
            json_of(&events)["events"]
                .as_array()
                .is_some_and(|e| e.len() >= 4)
        );

        let got_file = file(&lib, "pdf-forms", "2", "reference.md").expect("file");
        assert_eq!(got_file.status, 200);
        assert_eq!(got_file.bytes(), REFERENCE);
        assert!(
            got_file
                .headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v.starts_with("text/markdown"))
        );
        assert!(matches!(
            file(&lib, "pdf-forms", "2", "missing.md"),
            Err(StoreError::NotFound)
        ));

        let report = check(CheckBody {
            skill_md: GOOD.into(),
            files: vec![],
        })
        .expect("check");
        assert_eq!(report.status, 200);

        let rules = Response::json(200, &rules_body());
        assert_eq!(
            json_of(&rules)["rules_version"].as_u64(),
            Some(u64::from(RULES_VERSION))
        );
    }

    #[test]
    fn missing_and_invalid_cases() {
        let lib = library();
        let bus = Bus::default();
        create(&lib, &bus, new_skill(GOOD)).expect("create");

        // The same id again is a conflict.
        assert!(matches!(
            create(&lib, &bus, new_skill(GOOD)),
            Err(StoreError::Conflict(_))
        ));

        let mut bad_id = new_skill(GOOD);
        bad_id.id = Some("Not An Id".into());
        assert!(matches!(
            create(&lib, &bus, bad_id),
            Err(StoreError::Invalid(_))
        ));

        let mut bad_binding = new_skill(GOOD);
        bad_binding.id = Some("other".into());
        bad_binding.bindings = vec!["everyone".into()];
        assert!(matches!(
            create(&lib, &bus, bad_binding),
            Err(StoreError::Invalid(_))
        ));

        let mut bad_path = new_skill(GOOD);
        bad_path.id = Some("other".into());
        bad_path.files = vec![in_file("../x", Some(b64(b"x")), None)];
        assert!(matches!(
            create(&lib, &bus, bad_path),
            Err(StoreError::Invalid(_))
        ));

        assert!(matches!(detail(&lib, "missing"), Err(StoreError::NotFound)));
        assert!(matches!(
            version(&lib, "pdf-forms", "9"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            history(&lib, "missing"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            set_bindings(
                &lib,
                &bus,
                "pdf-forms",
                BindingsBody {
                    bindings: vec!["role:".into()]
                }
            ),
            Err(StoreError::Invalid(_))
        ));
        // A sha256 of a file the blobs do not hold is refused, not stored.
        assert!(matches!(
            add_version(
                &lib,
                &bus,
                "pdf-forms",
                version_body(
                    GOOD,
                    vec![in_file("reference.md", None, Some("0".repeat(64)))],
                    1
                )
            ),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn changes_publish_skill_events() {
        let lib = library();
        let bus = Bus::default();
        let events = bus.subscribe();

        create(&lib, &bus, new_skill(GOOD)).expect("create");
        let more = format!("{GOOD}\nMore.\n");
        add_version(
            &lib,
            &bus,
            "pdf-forms",
            version_body(
                &more,
                vec![in_file("reference.md", None, Some(sha256_hex(REFERENCE)))],
                1,
            ),
        )
        .expect("version");
        // The same SKILL.md and files again is not a new version, so it publishes nothing.
        add_version(
            &lib,
            &bus,
            "pdf-forms",
            version_body(
                &more,
                vec![in_file("reference.md", None, Some(sha256_hex(REFERENCE)))],
                2,
            ),
        )
        .expect("unchanged");
        publish(&lib, &bus, "pdf-forms", PublishBody { version: 2 }).expect("publish");
        set_bindings(
            &lib,
            &bus,
            "pdf-forms",
            BindingsBody {
                bindings: vec!["role:reviewer".into()],
            },
        )
        .expect("bindings");
        unpublish(&lib, &bus, "pdf-forms").expect("unpublish");
        archive(&lib, &bus, "pdf-forms", true).expect("archive");
        archive(&lib, &bus, "pdf-forms", false).expect("restore");

        let got: Vec<(String, String, String)> = events
            .try_iter()
            .map(|f| {
                let data: Value = serde_json::from_str(&f.data).expect("JSON data");
                (
                    f.kind,
                    data["id"].as_str().unwrap_or_default().to_string(),
                    data["action"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let actions: Vec<&str> = got.iter().map(|(_, _, action)| action.as_str()).collect();
        assert_eq!(
            actions,
            [
                "create",
                "version",
                "publish",
                "bindings",
                "unpublish",
                "archive",
                "restore"
            ]
        );
        assert!(
            got.iter()
                .all(|(kind, id, _)| kind == "skill" && id == "pdf-forms")
        );
    }
}
