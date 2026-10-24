//! The agents' artifact routes: `/internal/v1/agents/{cid}/gateway/artifacts…`. An agent makes an artifact, adds a
//! version to one it made, uploads the data blobs its panels read, and reads back its own artifacts. The files
//! routes' authorisation applies, and every create or update is checked by the laws; every write also counts against
//! the agent's daily budget, which the store checks under its own lock.

use std::collections::BTreeMap;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_view::artifact::{Artifact, Spec};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Budget, Refusal};
use crate::internal::authorized;
use crate::state::App;

/// Artifact writes (a version made, an artifact created) one agent may make in a day.
const DAILY_ARTIFACTS: u32 = 200;

/// Bytes of artifacts and blobs one agent may store in a day.
const DAILY_BYTES: u64 = 512 << 20;

/// One day in milliseconds: the window of the daily budget.
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The largest data blob an agent may upload in one request.
const MAX_BLOB: usize = 256 << 20;

/// The body of `PUT .../artifacts`: the spec, and the project it belongs to, if any.
#[derive(Deserialize)]
struct CreateBody {
    spec: Spec,
    project: Option<String>,
}

/// The body of `PUT .../artifacts/{id}`: the spec of the new version.
#[derive(Deserialize)]
struct UpdateBody {
    spec: Spec,
}

pub fn routes(router: Router<App>) -> Router<App> {
    // The blobs route is registered before `{id}` so that `blobs` is never read as an artifact id.
    router
        .route(
            "PUT",
            "/internal/v1/agents/{cid}/gateway/artifacts",
            create_route,
        )
        .doc(
            RouteDoc::new("Store an artifact an agent makes")
                .status(201)
                .errors(&[400, 403, 429]),
        )
        .route(
            "PUT",
            "/internal/v1/agents/{cid}/gateway/artifacts/blobs",
            blob_route,
        )
        .doc(
            RouteDoc::new("Store a data blob an artifact reads (?content_type=)")
                .status(201)
                .errors(&[400, 413, 429, 500]),
        )
        .route(
            "PUT",
            "/internal/v1/agents/{cid}/gateway/artifacts/{id}",
            update_route,
        )
        .doc(
            RouteDoc::new("Add a version to an artifact its author made")
                .status(201)
                .errors(&[400, 403, 404, 429]),
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/artifacts/{id}",
            get_route,
        )
        .doc(RouteDoc::new("Let an agent read an artifact it made (?v=)").errors(&[400, 404]))
}

/// `PUT .../artifacts`: a new artifact, checked by the laws and the daily budget, then stored and recorded.
fn create_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        let body: CreateBody = req
            .json()
            .map_err(|e| Response::error(400, &e.to_string()))?;
        // The laws judge the artifact by its id, which is reserved here so that the store can use it.
        let id = app.artifacts.reserve_id();
        if let Some(why) = refused_by_law(app, cid, &id, &body.spec) {
            return Err(Response::error(403, &why));
        }
        Ok(
            match app.artifacts.create_within(
                &id,
                &author_of(app, cid),
                Some(cid),
                body.project,
                body.spec,
                &daily_budget(),
            ) {
                Ok(artifact) => {
                    record(app, cid, &artifact);
                    Response::json(201, &reply(&artifact, false))
                }
                Err(refusal) => refusal_response(refusal),
            },
        )
    })
}

/// `PUT .../artifacts/{id}`: a new version of an artifact, for its author only, checked by the laws and the budget.
fn update_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        let id = p.get("id");
        let body: UpdateBody = req
            .json()
            .map_err(|e| Response::error(400, &e.to_string()))?;
        match app.artifacts.author_cid(id) {
            None => return Err(Response::error(404, "no such artifact")),
            Some(author) if author != cid => {
                return Err(Response::error(
                    403,
                    "only the artifact's author can add a version",
                ));
            }
            Some(_) => {}
        }
        if let Some(why) = refused_by_law(app, cid, id, &body.spec) {
            return Err(Response::error(403, &why));
        }
        Ok(
            match app
                .artifacts
                .update_within(id, Some(cid), body.spec, &daily_budget())
            {
                Ok(artifact) => {
                    record(app, cid, &artifact);
                    Response::json(201, &reply(&artifact, true))
                }
                Err(refusal) => refusal_response(refusal),
            },
        )
    })
}

/// `PUT .../artifacts/blobs?content_type=`: the raw body of a data blob, for an artifact to refer to.
fn blob_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        if app.agents.get(cid).is_none() {
            return Err(Response::error(404, "unknown agent"));
        }
        if req.body.is_empty() {
            return Err(Response::error(400, "the body is empty"));
        }
        if req.body.len() > MAX_BLOB {
            return Err(Response::error(
                413,
                &format!("a blob is at most {} MiB", MAX_BLOB >> 20),
            ));
        }
        let content_type = content_type_of(req);
        Ok(
            match app.artifacts.put_blob_within(
                Some(cid),
                &content_type,
                &req.body,
                &daily_budget(),
            ) {
                Ok(blob) => Response::json(201, &json!({ "blob": blob })),
                Err(Refusal::Budget(why)) => Response::error(429, &why),
                Err(Refusal::Problems(problems)) => Response::error(500, &problems.join("; ")),
            },
        )
    })
}

/// `GET .../artifacts/{id}?v=`: an artifact the agent made, at its newest or at version `v`. Any other artifact
/// answers 404, as the files route does for a file the agent cannot read.
fn get_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        let id = p.get("id");
        let version = match req.query_param("v") {
            None | Some("") => None,
            Some(raw) => match raw.parse::<u32>() {
                Ok(version) => Some(version),
                Err(_) => return Err(Response::error(400, "v must be a version number")),
            },
        };
        if app.artifacts.author_cid(id) != Some(cid) {
            return Err(Response::error(404, "no such artifact"));
        }
        Ok(match app.artifacts.get(id, version) {
            Some(artifact) => Response::json(200, &artifact),
            None => Response::error(404, "no such artifact"),
        })
    })
}

/// The 400 for a spec the store refuses, with the problems the agent can act on.
fn refused(problems: Vec<String>) -> Response {
    Response::json(
        400,
        &json!({ "error": "the artifact was refused", "problems": problems }),
    )
}

/// The response to a write the store refused: 429 when the daily budget is spent, else the 400 with the problems.
fn refusal_response(refusal: Refusal) -> Response {
    match refusal {
        Refusal::Budget(why) => Response::error(429, &why),
        Refusal::Problems(problems) => refused(problems),
    }
}

/// The reply to a stored artifact: its id, version, reference, kind and title. `versioned` names the reference
/// with its version too (`artifact:<id>@<version>`), as an update's reply does.
fn reply(a: &Artifact, versioned: bool) -> Value {
    let reference = if versioned {
        format!("artifact:{}@{}", a.id, a.version)
    } else {
        format!("artifact:{}", a.id)
    };
    json!({
        "id": a.id,
        "version": a.version,
        "ref": reference,
        "kind": a.kind,
        "title": a.title,
    })
}

/// The budget an agent's writes are checked against: the writes and bytes of the last day.
fn daily_budget() -> Budget {
    Budget {
        since_ms: apz_plaza::now_ms().saturating_sub(DAY_MS),
        check: over_budget,
    }
}

/// The budget's refusal for an agent that has made `count` writes of `bytes` in the last day, `None` while it has
/// room for one more. The limits are [`DAILY_ARTIFACTS`] and [`DAILY_BYTES`].
fn over_budget(count: u32, bytes: u64) -> Option<String> {
    if count < DAILY_ARTIFACTS && bytes < DAILY_BYTES {
        return None;
    }
    Some(format!(
        "artifact budget reached: {count} artifacts or {} MiB in 24 h",
        bytes >> 20
    ))
}

/// The laws' refusal of an artifact publish by agent `cid`, `None` when it may go. It is checked as the files route
/// checks a publish (`artifact.publish` on the artifact, by its id), so both kinds of publish face the same laws.
fn refused_by_law(app: &App, cid: u64, id: &str, spec: &Spec) -> Option<String> {
    let role = app
        .agents
        .get(cid)
        .map(|agent| agent.role)
        .unwrap_or_default();
    let resource = jhrl::authz::Entity {
        ty: "Artifact".into(),
        id: id.to_string(),
    };
    let input = serde_json::to_value(spec).unwrap_or(Value::Null);
    app.laws.refuse_op(
        cid,
        &role,
        "artifact.publish",
        resource,
        BTreeMap::new(),
        &input,
    )
}

/// The author an artifact of agent `cid` is recorded under: `a<cid> · <name>`, or `a<cid>` for an unnamed agent.
fn author_of(app: &App, cid: u64) -> String {
    match app.agents.get(cid).map(|agent| agent.role) {
        Some(role) => format!("a{cid} · {role}"),
        None => format!("a{cid}"),
    }
}

/// Records a stored artifact in the transcript of agent `cid`, under its current work (0 when it has none).
fn record(app: &App, cid: u64, artifact: &Artifact) {
    let work = app.work.current(cid).map_or(0, |item| item.id);
    app.transcripts.record(
        &app.bus,
        cid,
        work,
        crate::transcript::Entry::Artifact {
            id: artifact.id.clone(),
            version: artifact.version,
            title: artifact.title.clone(),
            kind: artifact.kind.clone(),
        },
    );
}

/// The content type of an uploaded blob: the `content_type` query (percent-decoded), else the request's own
/// content type, else `application/octet-stream`. The store checks it before it is served (see `safe_content_type`).
fn content_type_of(req: &Request) -> String {
    req.query_param("content_type")
        .map(percent_decode)
        .filter(|kind| !kind.trim().is_empty())
        .or_else(|| req.header("content-type").map(str::to_string))
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Percent-decoding of a query value. A `+` stays a plus, since MIME types such as `image/svg+xml` carry one; a
/// malformed escape is kept as it is.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = bytes
            .get(i + 1..i + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[i], escaped) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (other, _) => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(version: u32) -> Artifact {
        Artifact {
            id: "ar1".into(),
            version,
            latest: version,
            title: "Sales".into(),
            sub: String::new(),
            panels: Vec::new(),
            kind: "chart".into(),
            author: "a7 · Ada".into(),
            project: None,
            created_ms: 0,
            updated_ms: 0,
            bytes: 0,
        }
    }

    #[test]
    fn a_created_artifact_is_referenced_by_id_and_a_version_by_its_version() {
        assert_eq!(
            reply(&artifact(1), false),
            json!({"id": "ar1", "version": 1, "ref": "artifact:ar1", "kind": "chart", "title": "Sales"})
        );
        assert_eq!(reply(&artifact(3), true)["ref"], "artifact:ar1@3");
    }

    #[test]
    fn the_budget_refuses_at_the_daily_limits() {
        assert!(over_budget(0, 0).is_none());
        assert!(over_budget(DAILY_ARTIFACTS - 1, DAILY_BYTES - 1).is_none());
        assert!(over_budget(DAILY_ARTIFACTS, 0).is_some());
        assert!(over_budget(0, DAILY_BYTES).is_some());
        assert_eq!(
            over_budget(200, 3 << 20).as_deref(),
            Some("artifact budget reached: 200 artifacts or 3 MiB in 24 h")
        );
    }

    #[test]
    fn a_content_type_is_percent_decoded_and_keeps_its_plus() {
        assert_eq!(percent_decode("image%2Fsvg+xml"), "image/svg+xml");
        assert_eq!(
            percent_decode("text%2Fplain%3Bcharset%3Dutf-8"),
            "text/plain;charset=utf-8"
        );
        assert_eq!(percent_decode("a%2"), "a%2");
    }
}
