//! The owner share API: `/v1/shares…`. An owner creates, changes, lists, revokes and reads the events of the share
//! links to artifacts and Library files (docs/design/sharing.md). The public side that serves a link is
//! `shares::public`.

use apz_http::{Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Options, Share, Target, unix_ms};
use crate::state::App;

/// One day in milliseconds, for `expires_in_days`.
const DAY_MS: u64 = 86_400_000;

/// The most events one request returns.
const MAX_EVENTS: usize = 1_000;

/// The body of `POST /v1/shares`.
#[derive(Deserialize)]
struct CreateBody {
    target: Target,
    version: Option<u32>,
    password: Option<String>,
    expires_ms: Option<u64>,
    expires_in_days: Option<u64>,
    allow_download: Option<bool>,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/shares", |app, req, _| list(app, req))
        .doc(
            RouteDoc::new("List the share links, all of them or those of one target")
                .errors(&[400]),
        )
        .post("/v1/shares", |app, req, _| create(app, req))
        .doc(
            RouteDoc::new("Create a share link to an artifact or a Library file")
                .errors(&[400, 404]),
        )
        .route("PATCH", "/v1/shares/{id}", |app, req, p| {
            change(app, req, p.get("id"))
        })
        .doc(
            RouteDoc::new(
                "Change a share link: password, expiry, downloads, version (null clears)",
            )
            .errors(&[400, 404]),
        )
        .delete("/v1/shares/{id}", |app, req, p| {
            revoke(app, req, p.get("id"))
        })
        .doc(RouteDoc::new("Revoke a share link").errors(&[404, 500]))
        .get("/v1/shares/{id}/events", |app, req, p| {
            events(app, req, p.get("id"))
        })
        .doc(
            RouteDoc::new("The recent views, downloads and failed unlocks of a share link")
                .errors(&[404]),
        )
}

/// `GET /v1/shares[?target=artifact:<id>|file:<id>]`: the shares, newest first.
fn list(app: &App, req: &Request) -> Response {
    let target = match req.query_param("target") {
        None => None,
        Some(text) => match parse_target(text) {
            Some(target) => Some(target),
            None => return Response::error(400, "target is artifact:<id> or file:<id>"),
        },
    };
    let host = req.header("Host");
    let views: Vec<Value> = app
        .shares
        .list(target.as_ref())
        .iter()
        .map(|share| view(app, share, host))
        .collect();
    Response::json(200, &views)
}

/// `POST /v1/shares`: a share of an artifact or a Library file that exists. An artifact gets marked shared.
fn create(app: &App, req: &Request) -> Response {
    let body: CreateBody = match req.json() {
        Ok(body) => body,
        Err(_) => return Response::error(400, "the body is not valid JSON for this route"),
    };
    if let Err(msg) = check_target(&body.target) {
        return Response::error(400, msg);
    }
    if body.password.as_deref().is_some_and(short_password) {
        return Response::error(400, "a password is at least 8 characters");
    }
    if body.version.is_some() && body.target.kind != "artifact" {
        return Response::error(400, "version applies to artifacts only");
    }
    let Some(title) = target_title(app, &body.target) else {
        return Response::error(404, "no such artifact or file");
    };
    let now = unix_ms();
    let expires_ms = body.expires_ms.or_else(|| {
        body.expires_in_days
            .map(|days| now.saturating_add(days.saturating_mul(DAY_MS)))
    });
    let opts = Options {
        version: body.version,
        password: body.password,
        expires_ms,
        allow_download: body.allow_download.unwrap_or(false),
    };
    let share = match app.shares.create(body.target, title, opts, "owner") {
        Ok(share) => share,
        Err(_) => return Response::error(500, "the share could not be saved; try again"),
    };
    crate::shares::relay::claim(&share.id);
    if share.target.kind == "artifact" {
        app.artifacts.set_shared(&share.target.id, true);
    }
    Response::json(201, &view(app, &share, req.header("Host")))
}

/// `PATCH /v1/shares/{id}`: the fields present change. A `null` clears the password, the expiry or the version.
fn change(app: &App, req: &Request, id: &str) -> Response {
    let body: Value = match req.json() {
        Ok(body) => body,
        Err(_) => return Response::error(400, "the body is not valid JSON for this route"),
    };
    let Some(fields) = body.as_object() else {
        return Response::error(400, "the body is a JSON object");
    };
    let (password, expires_ms, allow_download, version) = match patch_fields(fields) {
        Ok(parsed) => parsed,
        Err(msg) => return Response::error(400, &msg),
    };
    if matches!(&password, Some(Some(p)) if short_password(p)) {
        return Response::error(400, "a password is at least 8 characters");
    }
    match app
        .shares
        .change(id, password, expires_ms, allow_download, version)
    {
        Ok(Some(share)) => Response::json(200, &view(app, &share, req.header("Host"))),
        Ok(None) => Response::error(404, "no such share"),
        Err(_) => Response::error(500, "the change could not be saved; try again"),
    }
}

/// `DELETE /v1/shares/{id}`: revokes the link. An artifact with no other live link is marked not shared again. A
/// revocation that cannot be saved answers 500, and the owner retries it.
fn revoke(app: &App, req: &Request, id: &str) -> Response {
    let share = match app.shares.revoke(id) {
        Ok(Some(share)) => share,
        Ok(None) => return Response::error(404, "no such share"),
        Err(_) => return Response::error(500, "the revocation could not be saved; try again"),
    };
    crate::shares::relay::release(&share.id);
    if share.target.kind == "artifact"
        && !app
            .shares
            .live()
            .iter()
            .any(|other| other.target == share.target)
    {
        app.artifacts.set_shared(&share.target.id, false);
    }
    Response::json(200, &view(app, &share, req.header("Host")))
}

/// `GET /v1/shares/{id}/events[?limit=n]`: the recent events of a share, newest first.
fn events(app: &App, req: &Request, id: &str) -> Response {
    if app.shares.get(id).is_none() {
        return Response::error(404, "no such share");
    }
    let limit = req
        .query_param("limit")
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, MAX_EVENTS);
    Response::json(200, &app.shares.events(id, limit))
}

/// The owner's view of a share: every field but the password hash, with `has_password`, the counts, the state
/// and the links. `host` is the request's Host header, which the network link uses; without one there is no
/// network link. The public link is on agentplaza.io.
fn view(app: &App, share: &Share, host: Option<&str>) -> Value {
    let (views, downloads) = app.shares.counts(&share.id);
    let scheme = if app.config.tls { "https" } else { "http" };
    let network = host.map(|host| format!("{scheme}://{host}/artifact/{}", share.id));
    json!({
        "id": share.id,
        "target": share.target,
        "version": share.version,
        "has_password": share.password_hash.is_some(),
        "expires_ms": share.expires_ms,
        "allow_download": share.allow_download,
        "created_by": share.created_by,
        "created_ms": share.created_ms,
        "revoked_ms": share.revoked_ms,
        "title": share.title,
        "views": views,
        "downloads": downloads,
        "state": state_name(share, unix_ms()),
        "links": {
            "public": format!("https://agentplaza.io/artifact/{}", share.id),
            "network": network,
        },
    })
}

/// "revoked", "expired" or "live" for `share` at `now`.
fn state_name(share: &Share, now: u64) -> &'static str {
    if share.revoked_ms.is_some() {
        "revoked"
    } else if share.expires_ms.is_some_and(|at| now >= at) {
        "expired"
    } else {
        "live"
    }
}

/// A PATCH field: absent is `None` (unchanged), `null` is `Some(None)` (cleared), and a value `parse` reads is
/// `Some(Some(value))`. A value of another type is an error naming the field.
fn optional<T>(
    fields: &Map<String, Value>,
    key: &str,
    parse: impl Fn(&Value) -> Option<T>,
) -> Result<Option<Option<T>>, String> {
    match fields.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) => match parse(value) {
            Some(parsed) => Ok(Some(Some(parsed))),
            None => Err(format!("{key} has the wrong type, or is null to clear it")),
        },
    }
}

/// The fields a PATCH may change, as `Store::change` takes them (see [`optional`]).
type Patch = (
    Option<Option<String>>,
    Option<Option<u64>>,
    Option<bool>,
    Option<Option<u32>>,
);

/// The PATCH fields of a request body: password, expires_ms, allow_download and version.
fn patch_fields(fields: &Map<String, Value>) -> Result<Patch, String> {
    let password = optional(fields, "password", |v| v.as_str().map(str::to_string))?;
    let expires_ms = optional(fields, "expires_ms", Value::as_u64)?;
    let allow_download = match fields.get("allow_download") {
        None => None,
        Some(v) => Some(
            v.as_bool()
                .ok_or_else(|| "allow_download is true or false".to_string())?,
        ),
    };
    let version = optional(fields, "version", |v| {
        v.as_u64().and_then(|n| u32::try_from(n).ok())
    })?;
    Ok((password, expires_ms, allow_download, version))
}

/// Whether `password` is too short to set. A password is at least 8 characters; the empty string clears one.
fn short_password(password: &str) -> bool {
    !password.is_empty() && password.chars().count() < 8
}

/// The target of a request: `kind` is "artifact" or "file", and `id` is not empty.
fn check_target(target: &Target) -> Result<(), &'static str> {
    match target.kind.as_str() {
        "artifact" | "file" if !target.id.is_empty() => Ok(()),
        _ => Err("target is {kind: artifact or file, id: the id}"),
    }
}

/// A target written as `artifact:<id>` or `file:<id>` (the `target` query parameter).
fn parse_target(text: &str) -> Option<Target> {
    let (kind, id) = text.split_once(':')?;
    let target = Target {
        kind: kind.to_string(),
        id: id.to_string(),
    };
    check_target(&target).ok().map(|()| target)
}

/// The title of the artifact or Library file `target` names, or `None` when there is none.
fn target_title(app: &App, target: &Target) -> Option<String> {
    match target.kind.as_str() {
        "artifact" => app
            .artifacts
            .get(&target.id, None)
            .map(|artifact| artifact.title),
        "file" => app.deliverables.find(&target.id).map(|file| file.title),
        _ => None,
    }
}
