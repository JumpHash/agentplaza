//! Share requests: an agent asks the owner to share an artifact it made outside the plaza. The request is an approval
//! of kind `artifact.share`, so the owner decides it in Approvals. An approval creates the share, marks the artifact
//! shared and claims the share with the relay. The agent hears the decision in its transcript and never gets the link.

use std::collections::BTreeMap;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::approvals::{self, Approval, State};
use crate::internal::authorized;
use crate::shares::{Options, Target, unix_ms};
use crate::state::App;

/// The longest reason an agent may give, in characters.
const MAX_REASON: usize = 500;

/// The longest share an owner may approve, in days.
const MAX_DAYS: u64 = 365;

/// One day in milliseconds, for `expires_in_days`.
const DAY_MS: u64 = 86_400_000;

/// The body of `POST .../artifacts/{id}/share`.
#[derive(Deserialize)]
struct RequestBody {
    reason: String,
    expires_in_days: Option<u64>,
    allow_download: Option<bool>,
    version: Option<u32>,
}

/// The share an approved request creates: its target, title, options and the `created_by` it is recorded under.
#[derive(Debug)]
struct Plan {
    target: Target,
    title: String,
    options: Options,
    created_by: String,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .route(
            "POST",
            "/internal/v1/agents/{cid}/gateway/artifacts/{id}/share",
            share_route,
        )
        .doc(
            RouteDoc::new("Ask the owner to share an artifact the agent made")
                .status(202)
                .errors(&[400, 403, 404]),
        )
}

/// `POST .../artifacts/{id}/share`: a request for the owner to share an artifact. Only its author may ask. The laws
/// may refuse the request; otherwise it is queued for the owner and the agent gets the approval's id.
fn share_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        let id = p.get("id");
        let body: RequestBody = req
            .json()
            .map_err(|e| Response::error(400, &e.to_string()))?;
        validate(&body).map_err(|why| Response::error(400, &why))?;
        match app.artifacts.author_cid(id) {
            None => return Err(Response::error(404, "no such artifact")),
            Some(author) if author != cid => {
                return Err(Response::error(
                    403,
                    "only the artifact's author can ask to share it",
                ));
            }
            Some(_) => {}
        }
        let Some(artifact) = app.artifacts.get(id, None) else {
            return Err(Response::error(404, "no such artifact"));
        };
        let detail = request_detail(
            id,
            &artifact.title,
            &artifact.kind,
            &body,
            &agent_of(app, cid),
            cid,
        );
        if let Some(why) = refused_by_law(app, cid, id, &detail) {
            return Err(Response::error(403, &why));
        }
        let item = approvals::submit(app, "artifact.share", &artifact.title, detail);
        record(
            app,
            cid,
            "share_request",
            json!({"artifact": id, "title": artifact.title, "approval": item.id}),
        );
        Ok(Response::json(
            202,
            &json!({"approval": item.id, "state": "pending"}),
        ))
    })
}

/// Acts on the owner's decision on a share request, as the approvals store records it. An approval creates the share,
/// marks the artifact shared and claims the share with the relay. A denial creates nothing. Either way the agent's
/// transcript gets a line saying what the owner decided, and the link is never written there.
pub fn on_decided(app: &App, approval: &Approval) -> Result<(), String> {
    let approved = approval.state == State::Approved;
    let title = approval.detail["artifact"]["title"]
        .as_str()
        .unwrap_or(approval.subject.as_str())
        .to_string();
    let text = match plan(&approval.detail, approved, unix_ms()) {
        Some(plan) if app.artifacts.get(&plan.target.id, None).is_some() => {
            let id = plan.target.id.clone();
            let share = app
                .shares
                .create(plan.target, plan.title, plan.options, &plan.created_by);
            app.artifacts.set_shared(&id, true);
            crate::shares::relay::claim(&share.id);
            format!("The owner approved sharing {title}.")
        }
        _ if approved => format!(
            "The owner approved sharing {title}, but the artifact is no longer there, so no link was made."
        ),
        _ => format!("The owner declined sharing {title}."),
    };
    record(
        app,
        approval.cid,
        "share_decision",
        json!({"text": text, "approved": approved, "title": title}),
    );
    Ok(())
}

/// The checks on a share request: the reason is required and at most `MAX_REASON` characters, `expires_in_days` is
/// between 1 and `MAX_DAYS`, and a version starts at 1. The first failure is returned as the 400's message.
fn validate(body: &RequestBody) -> Result<(), String> {
    let reason = body.reason.trim();
    if reason.is_empty() {
        return Err("reason is required: say why the owner should share this artifact".to_string());
    }
    if reason.chars().count() > MAX_REASON {
        return Err(format!("reason is at most {MAX_REASON} characters"));
    }
    if body
        .expires_in_days
        .is_some_and(|days| !(1..=MAX_DAYS).contains(&days))
    {
        return Err(format!("expires_in_days is between 1 and {MAX_DAYS}"));
    }
    if body.version == Some(0) {
        return Err("version is a version number, starting at 1".to_string());
    }
    Ok(())
}

/// The approval's detail: the artifact, the options asked for, the reason, and the agent that asks. It carries no
/// share, link, token or password; the share is made only when the owner approves.
fn request_detail(
    id: &str,
    title: &str,
    kind: &str,
    body: &RequestBody,
    agent: &str,
    cid: u64,
) -> Value {
    json!({
        "artifact": {"id": id, "title": title, "kind": kind},
        "options": {
            "version": body.version,
            "expires_in_days": body.expires_in_days,
            "allow_download": body.allow_download.unwrap_or(false),
        },
        "reason": body.reason.trim(),
        "agent": agent,
        "cid": cid,
    })
}

/// The share an approval creates, from the approval's detail, or `None` when the owner denied it. The expiry counts
/// from `now`, the time of the approval, not of the request.
fn plan(detail: &Value, approved: bool, now: u64) -> Option<Plan> {
    if !approved {
        return None;
    }
    let id = detail["artifact"]["id"].as_str()?;
    let title = detail["artifact"]["title"].as_str()?.to_string();
    let options = &detail["options"];
    let agent = detail["agent"].as_str().unwrap_or("an agent");
    Some(Plan {
        target: Target {
            kind: "artifact".into(),
            id: id.to_string(),
        },
        title,
        options: Options {
            version: options["version"]
                .as_u64()
                .and_then(|version| u32::try_from(version).ok()),
            password: None,
            expires_ms: options["expires_in_days"]
                .as_u64()
                .map(|days| now.saturating_add(days.saturating_mul(DAY_MS))),
            allow_download: options["allow_download"].as_bool().unwrap_or(false),
        },
        created_by: format!("owner, asked by {agent}"),
    })
}

/// The laws' refusal of a share request by agent `cid`, `None` when it may go.
fn refused_by_law(app: &App, cid: u64, id: &str, input: &Value) -> Option<String> {
    let role = app
        .agents
        .get(cid)
        .map(|agent| agent.role)
        .unwrap_or_default();
    let resource = jhrl::authz::Entity {
        ty: "Artifact".into(),
        id: id.to_string(),
    };
    app.laws.refuse_op(
        cid,
        &role,
        "artifact.share",
        resource,
        BTreeMap::new(),
        input,
    )
}

/// The label the owner sees for agent `cid`: its role and CID, or `a<cid>` for one not on the roster.
fn agent_of(app: &App, cid: u64) -> String {
    app.agents
        .get(cid)
        .map(|agent| agent.label())
        .unwrap_or_else(|| format!("a{cid}"))
}

/// Records a line in the transcript of agent `cid`, under its current work (0 when it has none).
fn record(app: &App, cid: u64, kind: &'static str, data: Value) {
    let work = app.work.current(cid).map_or(0, |item| item.id);
    app.transcripts.record(&app.bus, cid, work, kind, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::Store;

    fn body(value: Value) -> RequestBody {
        serde_json::from_value(value).expect("a request body")
    }

    fn sales_detail(approved_days: u64) -> Value {
        request_detail(
            "ar1",
            "Sales",
            "chart",
            &body(
                json!({"reason": "for the board", "expires_in_days": approved_days, "allow_download": true}),
            ),
            "a7 · Ada",
            7,
        )
    }

    #[test]
    fn the_options_are_validated() {
        assert!(validate(&body(json!({"reason": "for the board"}))).is_ok());
        assert!(
            validate(&body(json!({"reason": "   "}))).is_err(),
            "a blank reason is refused"
        );
        assert!(validate(&body(json!({"reason": "x".repeat(500)}))).is_ok());
        assert!(validate(&body(json!({"reason": "x".repeat(501)}))).is_err());
        assert!(validate(&body(json!({"reason": "r", "expires_in_days": 0}))).is_err());
        assert!(validate(&body(json!({"reason": "r", "expires_in_days": 366}))).is_err());
        assert!(
            validate(&body(
                json!({"reason": "r", "expires_in_days": 365, "version": 2})
            ))
            .is_ok()
        );
        assert!(validate(&body(json!({"reason": "r", "version": 0}))).is_err());
    }

    #[test]
    fn the_approval_detail_carries_no_link() {
        let detail = sales_detail(7);
        assert_eq!(
            detail["artifact"],
            json!({"id": "ar1", "title": "Sales", "kind": "chart"})
        );
        assert_eq!(detail["options"]["expires_in_days"], 7);
        assert_eq!(detail["options"]["allow_download"], true);
        assert_eq!(detail["agent"], "a7 · Ada");
        assert_eq!(detail["cid"], 7);
        let text = detail.to_string();
        for word in [
            "http",
            "/artifact/",
            "token",
            "password",
            "\"url\"",
            "\"link\"",
        ] {
            assert!(!text.contains(word), "{word} must not be in the detail");
        }
    }

    #[test]
    fn a_denial_plans_no_share_and_an_approval_plans_one() {
        let detail = sales_detail(7);
        assert!(plan(&detail, false, 1_000).is_none());
        let planned = plan(&detail, true, 1_000).expect("an approval plans a share");
        assert_eq!(
            planned.target,
            Target {
                kind: "artifact".into(),
                id: "ar1".into()
            }
        );
        assert_eq!(planned.title, "Sales");
        assert_eq!(planned.created_by, "owner, asked by a7 · Ada");
        assert_eq!(planned.options.expires_ms, Some(1_000 + 7 * DAY_MS));
        assert!(planned.options.allow_download);
        assert!(planned.options.password.is_none());
    }

    #[test]
    fn an_approved_request_creates_exactly_one_live_share() {
        let dir = std::env::temp_dir().join(format!("apid-share-request-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test folder");
        let store = Store::load(&dir);
        let detail = sales_detail(30);

        // The denial plans nothing, so nothing is created.
        assert!(plan(&detail, false, unix_ms()).is_none());
        assert!(store.live().is_empty());

        let planned = plan(&detail, true, unix_ms()).expect("an approval plans a share");
        let share = store.create(
            planned.target,
            planned.title,
            planned.options,
            &planned.created_by,
        );
        let live = store.live();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, share.id);
        assert_eq!(live[0].target.id, "ar1");
        assert!(live[0].expires_ms.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
