//! The preview routes (PLAN 18.1, 18.2): `/preview/<project>/<lane>/…`, the hub's public path for a lane's app, which
//! checks its own access (the owner's sign-in, or a link); and the owner's `/v1/` routes that share, revoke, audit and
//! put a preview to sleep.
//!
//! A link's first request carries its token as `preview_token` in the query: the hub answers with a redirect that sets
//! the token as a cookie scoped to the preview's path and drops it from the address. Every request to the preview
//! after that carries the cookie. A link grants its preview and nothing else of the hub: the token is checked against
//! the preview of the path, and the hub's other routes are never reached with it.

use apz_http::{Body, Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::base_vm::{Host, Live};
use crate::hub_auth;
use crate::previews::life::{self, Ctx};
use crate::previews::{
    ASLEEP, CLOSED, COOKIE, Link, Preview, Store, TOKEN_PARAM, USE, VIEW, proxy,
};
use crate::state::App;

/// The longest a link may last: 90 days.
const MAX_LINK_S: u64 = 90 * 24 * 3600;
/// A link's expiry when the request names none: 7 days.
const DEFAULT_LINK_S: u64 = 7 * 24 * 3600;
/// How many openings the owner's list shows at most.
const OPENINGS_PAGE: u32 = 200;

pub fn routes(router: Router<App>) -> Router<App> {
    let mut router = router;
    for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
        router = router
            .route(
                method,
                "/preview/{project}/{lane}/",
                |app: &App, req: &Request, p: &Params<'_>| serve(app, req, p),
            )
            .doc(RouteDoc::new(
                "Serve a lane's preview: the app, to the owner or to a link's token",
            ))
            .route(
                method,
                "/preview/{project}/{lane}/{rest..}",
                |app: &App, req: &Request, p: &Params<'_>| serve(app, req, p),
            )
            .doc(RouteDoc::new(
                "Serve a file of a lane's preview: the app, to the owner or to a link's token",
            ));
    }
    router
        .get("/v1/projects/{project}/previews", |app, _, p| {
            Response::json(
                200,
                &json!({"previews": previews_json(app, p.get("project"))}),
            )
        })
        .doc(RouteDoc::new("List a project's previews"))
        .post("/v1/previews/{id}/links", |app, req, p| {
            create_link(app, req, p.get("id"))
        })
        .doc(
            RouteDoc::new("Share a preview: a link with a scope, a label and an expiry")
                .errors(&[400, 404]),
        )
        .get("/v1/previews/{id}/links", |app, _, p| {
            list_links(app, p.get("id"))
        })
        .doc(RouteDoc::new("List a preview's links").errors(&[404]))
        .delete("/v1/preview-links/{id}", |app, _, p| {
            revoke(app, p.get("id"))
        })
        .doc(RouteDoc::new("Revoke a preview link: it never works again").errors(&[404]))
        .get("/v1/preview-links/{id}/openings", |app, _, p| {
            openings(app, p.get("id"))
        })
        .doc(RouteDoc::new("List a link's openings: when, from where, what").errors(&[404]))
        .get("/v1/preview-links/{id}/qr", |app, _, p| {
            qr(app, p.get("id"))
        })
        .doc(RouteDoc::new("The QR code of a preview link's address, as SVG").errors(&[404, 500]))
        .post("/v1/previews/{id}/sleep", |app, _, p| {
            vm_action(app, p.get("id"), Action::Sleep)
        })
        .doc(RouteDoc::new("Put a preview to sleep now").errors(&[404, 409, 503]))
        .post("/v1/previews/{id}/wake", |app, _, p| {
            vm_action(app, p.get("id"), Action::Wake)
        })
        .doc(RouteDoc::new("Wake a sleeping preview").errors(&[404, 409, 503]))
        .post("/v1/previews/{id}/close", |app, _, p| {
            vm_action(app, p.get("id"), Action::Close)
        })
        .doc(RouteDoc::new("Close a preview now, before its lane ends").errors(&[404, 503]))
}

/// The public path: the owner, or a link to this preview. Everything else is refused before the app is reached.
fn serve(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let owner = hub_auth::is_owner(app, req);
    let host = Live(app);
    let ctx = Ctx {
        store: &app.previews,
        nodes: &app.nodes,
        host: &host,
    };
    answer(
        &ctx,
        req,
        owner,
        p.get("project"),
        p.get("lane"),
        p.get("rest"),
        apz_plaza::now_ms(),
    )
}

/// Answers a request to lane `lane` of `project` at `rest` (the path after the lane). `owner` says the request is the
/// owner's. A link is checked against this preview, and each opening is recorded before the app is reached.
pub fn answer<H: Host>(
    ctx: &Ctx<H>,
    req: &Request,
    owner: bool,
    project: &str,
    lane: &str,
    rest: &str,
    now: u64,
) -> Response {
    let Some(mut preview) = ctx.store.lane_preview(project, lane) else {
        return Response::error(404, "no preview of that lane");
    };
    if preview.status == CLOSED {
        return Response::error(410, "the lane's preview has ended");
    }
    let (query_token, query) = take_token(&req.query);
    let path = format!("/{rest}");
    if !owner {
        let Some(token) = query_token.clone().or_else(|| cookie(req, COOKIE)) else {
            return Response::error(401, "sign in, or open a preview link");
        };
        let link = match admit(ctx.store, &token, &preview.id, &req.method, now) {
            Ok(link) => link,
            Err((status, why)) => return Response::error(status, why),
        };
        let from = req.peer.map(|peer| peer.to_string()).unwrap_or_default();
        if let Err(why) =
            ctx.store
                .record_opening(&link.id, &preview.id, &from, &req.method, &path, now)
        {
            return Response::error(500, &format!("the opening was not recorded: {why}"));
        }
        if query_token.is_some() {
            return token_redirect(project, lane, rest, &query, &token);
        }
    }
    if preview.status == ASLEEP
        && let Err(why) = life::wake(ctx, &mut preview)
    {
        return Response::error(503, &format!("the preview did not wake: {why}"));
    }
    preview.used_ms = now;
    if let Err(why) = ctx.store.save_preview(&preview) {
        eprintln!("previews: the idle clock of {}: {why}", preview.id);
    }
    proxy::fetch(
        ctx.nodes,
        &preview.node,
        preview.cid,
        preview.port,
        req,
        &path,
        &query,
    )
}

/// Checks a link's token for `preview` at `method` and `now`: the refusal's status and reason when it does not admit.
fn admit(
    store: &Store,
    token: &str,
    preview: &str,
    method: &str,
    now: u64,
) -> Result<Link, (u16, &'static str)> {
    let refused = (401, "that link is not valid");
    let id = store.verified_id(token).ok_or(refused)?;
    let link = store.link(&id).ok_or(refused)?;
    if link.preview != preview {
        return Err((401, "that link is for another preview"));
    }
    if link.revoked_ms.is_some() {
        return Err((401, "that link was revoked"));
    }
    if link.expires_ms.is_some_and(|at| now >= at) {
        return Err((401, "that link has expired"));
    }
    if link.scope == VIEW && !matches!(method, "GET" | "HEAD") {
        return Err((403, "that link only views the preview"));
    }
    Ok(link)
}

/// The redirect that keeps a link's token out of the address: the cookie for the preview's path, the request's own
/// address without the token.
fn token_redirect(project: &str, lane: &str, rest: &str, query: &str, token: &str) -> Response {
    let base = format!("/preview/{project}/{lane}/{rest}");
    let location = if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    };
    let cookie =
        format!("{COOKIE}={token}; Path=/preview/{project}/{lane}/; HttpOnly; SameSite=Lax");
    Response::new(303, "text/plain", "")
        .with_header("location", &location)
        .with_header("set-cookie", &cookie)
}

/// Splits the link's token from a query: the token, and the rest of the query.
pub(crate) fn take_token(query: &str) -> (Option<String>, String) {
    let mut token = None;
    let mut kept = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        match pair.split_once('=') {
            Some((name, value)) if name == TOKEN_PARAM => token = Some(value.to_string()),
            _ => kept.push(pair),
        }
    }
    (token, kept.join("&"))
}

/// The value of cookie `name` in the request, if it has one.
fn cookie(req: &Request, name: &str) -> Option<String> {
    req.header("cookie")?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

#[derive(Deserialize)]
struct NewLink {
    scope: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    expires_in_s: Option<u64>,
}

fn create_link(app: &App, req: &Request, id: &str) -> Response {
    let Some(preview) = app.previews.preview(id) else {
        return Response::error(404, &format!("no preview {id}"));
    };
    if preview.status == CLOSED {
        return Response::error(409, "the preview has ended: nothing to share");
    }
    let new: NewLink = match serde_json::from_slice(&req.body) {
        Ok(new) => new,
        Err(why) => return Response::error(400, &format!("a link needs a scope: {why}")),
    };
    if new.scope != VIEW && new.scope != USE {
        return Response::error(400, "the scope is view or use");
    }
    let lasts = new.expires_in_s.unwrap_or(DEFAULT_LINK_S);
    if lasts == 0 || lasts > MAX_LINK_S {
        return Response::error(400, "a link lasts between 1 second and 90 days");
    }
    let now = apz_plaza::now_ms();
    let label: String = new.label.trim().chars().take(80).collect();
    match app.previews.add_link(
        &preview.id,
        &new.scope,
        &label,
        Some(now + lasts * 1000),
        "owner",
        now,
    ) {
        Ok(link) => {
            let token = app.previews.mint(&link.id);
            Response::json(
                200,
                &json!({
                    "link": link_json(&link, now),
                    "token": token,
                    "url": share_url(app, &preview, &token),
                    "qr": format!("/v1/preview-links/{}/qr", link.id),
                }),
            )
        }
        Err(why) => Response::error(500, &format!("the link was not saved: {why}")),
    }
}

fn list_links(app: &App, id: &str) -> Response {
    if app.previews.preview(id).is_none() {
        return Response::error(404, &format!("no preview {id}"));
    }
    let now = apz_plaza::now_ms();
    let links: Vec<Value> = app
        .previews
        .links(id)
        .iter()
        .map(|link| link_json(link, now))
        .collect();
    Response::json(200, &json!({"links": links}))
}

fn revoke(app: &App, id: &str) -> Response {
    match app.previews.revoke(id, apz_plaza::now_ms()) {
        Ok(Some(link)) => {
            Response::json(200, &json!({"link": link_json(&link, apz_plaza::now_ms())}))
        }
        Ok(None) => Response::error(404, &format!("no link {id}")),
        Err(why) => Response::error(500, &format!("the revocation was not saved: {why}")),
    }
}

fn openings(app: &App, id: &str) -> Response {
    if app.previews.link(id).is_none() {
        return Response::error(404, &format!("no link {id}"));
    }
    let rows: Vec<Value> = app
        .previews
        .openings(id, OPENINGS_PAGE)
        .iter()
        .map(|o| json!({"at_ms": o.at_ms, "from": o.from, "method": o.method, "path": o.path}))
        .collect();
    Response::json(200, &json!({"openings": rows}))
}

/// The QR code of a link's address, as SVG: the address the owner shares, token included.
fn qr(app: &App, id: &str) -> Response {
    let Some(link) = app.previews.link(id) else {
        return Response::error(404, &format!("no link {id}"));
    };
    let Some(preview) = app.previews.preview(&link.preview) else {
        return Response::error(404, "the link's preview is gone");
    };
    let url = share_url(app, &preview, &app.previews.mint(&link.id));
    match qr_svg(&url) {
        Ok(svg) => Response {
            status: 200,
            headers: vec![
                ("Content-Type".to_string(), "image/svg+xml".to_string()),
                ("Cache-Control".to_string(), "no-store".to_string()),
            ],
            body: Body::Bytes(svg.into_bytes()),
        },
        Err(why) => Response::error(500, &why),
    }
}

fn qr_svg(url: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|why| format!("the QR code: {why}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .build())
}

enum Action {
    Sleep,
    Wake,
    Close,
}

fn vm_action(app: &App, id: &str, action: Action) -> Response {
    let Some(mut preview) = app.previews.preview(id) else {
        return Response::error(404, &format!("no preview {id}"));
    };
    let host = Live(app);
    let ctx = Ctx {
        store: &app.previews,
        nodes: &app.nodes,
        host: &host,
    };
    let result = match action {
        Action::Sleep => life::sleep(&ctx, &mut preview),
        Action::Wake => life::wake(&ctx, &mut preview),
        Action::Close => life::close(&ctx, &mut preview),
    };
    match result {
        Ok(()) => Response::json(200, &json!({"preview": preview_json(&preview)})),
        Err(why) => Response::error(503, &why),
    }
}

fn previews_json(app: &App, project: &str) -> Vec<Value> {
    app.previews
        .previews(Some(project))
        .iter()
        .map(preview_json)
        .collect()
}

fn preview_json(preview: &Preview) -> Value {
    json!({
        "id": preview.id,
        "project": preview.project,
        "lane": preview.lane,
        "node": preview.node,
        "status": preview.status,
        "port": preview.port,
        "created_ms": preview.created_ms,
        "used_ms": preview.used_ms,
        "ends_ms": preview.ends_ms,
        "path": format!("/preview/{}/{}/", preview.project, preview.lane),
    })
}

fn link_json(link: &Link, now: u64) -> Value {
    let state = if link.revoked_ms.is_some() {
        "revoked"
    } else if link.expires_ms.is_some_and(|at| now >= at) {
        "expired"
    } else {
        "active"
    };
    json!({
        "id": link.id,
        "preview": link.preview,
        "scope": link.scope,
        "label": link.label,
        "created_by": link.created_by,
        "created_ms": link.created_ms,
        "expires_ms": link.expires_ms,
        "revoked_ms": link.revoked_ms,
        "state": state,
    })
}

/// The address a link shares: the hub's public address, the preview's path, and the token in the query.
fn share_url(app: &App, preview: &Preview, token: &str) -> String {
    format!(
        "{}/preview/{}/{}/?{TOKEN_PARAM}={token}",
        app.config.public_url.trim_end_matches('/'),
        preview.project,
        preview.lane
    )
}
