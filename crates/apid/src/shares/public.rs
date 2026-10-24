//! The public share routes `/artifact/{id}…`, outside `/v1/`: each checks the share itself.
//!
//! Every path outside `/v1/` passes the owner gate (`hub_auth::is_public`), so nothing here relies on the owner
//! token. A share id must be 32 lowercase hex characters, and every other request answers as a missing share. The
//! viewer page is static; it reads `/data` for the share's content and theme.

use std::path::PathBuf;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_theme::appearance::ModeChoice;
use apz_theme::builtin;
use apz_theme::generate::theme_css;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::content;
use crate::shares::{Access, Share, Store, UNLOCK_TTL_MS, relay, unix_ms};
use crate::state::App;

/// The `Content-Security-Policy` of a frame: sandboxed without `allow-same-origin`, so its scripts never share the
/// hub's origin. A frame is an opaque origin and cannot read the viewer's cookies or storage.
const FRAME_SANDBOX: &str = "sandbox allow-scripts allow-popups allow-downloads";

/// The ancestors that may embed a frame: the hub itself and the relay.
const FRAME_ANCESTORS: &str = "'self' https://agentplaza.io";

/// The policy of a blob or a file served from a share: no script, no embedding, sandboxed.
const SANDBOXED: &str = "default-src 'none'; sandbox";

/// The origin the relay serves shares from.
const RELAY_ORIGIN: &str = "https://agentplaza.io";

/// The unlock form: the password a viewer types.
#[derive(Deserialize)]
struct Unlock {
    password: String,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/artifact/_assets/{file}", assets)
        .doc(RouteDoc::new("Serve a file of the share viewer").errors(&[404]))
        .get("/artifact/{share}", viewer)
        .doc(RouteDoc::new("Serve the share viewer page"))
        .get("/artifact/{share}/data", data_route)
        .doc(RouteDoc::new("Read a shared artifact or file and its theme").errors(&[401, 404, 410]))
        .post("/artifact/{share}/unlock", unlock_route)
        .doc(RouteDoc::new("Unlock a password-protected share").errors(&[400, 403, 404, 410]))
        .get("/artifact/{share}/blobs/{blob}", blob_route)
        .doc(RouteDoc::new("Read a data blob the shared version references").errors(&[401, 404]))
        .get("/artifact/{share}/rows", rows_route)
        .doc(RouteDoc::new("Read a page of a shared dataset's rows").errors(&[400, 401, 404]))
        .get("/artifact/{share}/export", export_route)
        .doc(RouteDoc::new("Export a shared artifact or dataset").errors(&[400, 401, 403, 404]))
        .get("/artifact/{share}/file", file_route)
        .doc(RouteDoc::new("Read a shared file").errors(&[401, 403, 404]))
        .get("/artifact/{share}/frame/{panel}/{path..}", frame_route)
        .doc(RouteDoc::new("Serve a file of a panel in a sandboxed frame").errors(&[401, 404]))
}

/// `GET /artifact/_assets/{file}`: a file of the viewer, from the console's build folder (or its `assets` folder).
fn assets(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let file = p.get("file");
    if safe_name(file) != file {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    }
    let root = &app.config.console;
    let found = [root.join(file), root.join("assets").join(file)]
        .into_iter()
        .find(|path| path.is_file());
    let Some(path) = found else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    match std::fs::read(&path) {
        Ok(bytes) => secure(
            Response::new(200, asset_type(file), bytes),
            "public, max-age=31536000, immutable",
        ),
        Err(_) => no_store(Response::json(404, &json!({"error": "missing"}))),
    }
}

/// `GET /artifact/{share}`: the viewer page. It answers 200 even for a missing share, which the page then shows.
fn viewer(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let page: PathBuf = app.config.console.join("viewer.html");
    let Ok(bytes) = std::fs::read(&page) else {
        return no_store(Response::json(
            404,
            &json!({"error": "the viewer is not built"}),
        ));
    };
    let share = if valid_id(id) {
        app.shares.get(id)
    } else {
        None
    };
    let response = Response::new(200, "text/html; charset=utf-8", bytes)
        .with_header("content-security-policy", &viewer_csp(&content_origin(req)));
    finish(response, share.as_ref())
}

/// `GET /artifact/{share}/data`: the share's artifact (or file) at its version, and the owner's theme. Records a view.
fn data_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let share = match admit(app, id, unlocked(app, req, id)) {
        Ok(share) => share,
        Err(refused) => return refused,
    };
    let mut body = if share.target.kind == "file" {
        let Some(item) = app.deliverables.find(&share.target.id) else {
            return no_store(Response::json(404, &json!({"error": "missing"})));
        };
        json!({
            "share": share_value(&share, None),
            "file": {
                "name": safe_name(&item.name),
                "content_type": item.content_type,
                "bytes": item.size,
            },
        })
    } else {
        let Some(artifact) = app.artifacts.get(&share.target.id, share.version) else {
            return no_store(Response::json(404, &json!({"error": "missing"})));
        };
        json!({
            "share": share_value(&share, Some(artifact.latest)),
            "artifact": artifact,
        })
    };
    body["theme"] = theme_value(app);
    app.shares.record(&share.id, "view", via(req));
    finish(Response::json(200, &body), Some(&share))
}

/// `POST /artifact/{share}/unlock`: checks the password and gives back the unlock cookie. A wrong password is
/// recorded and answers 403.
fn unlock_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let access = if valid_id(id) {
        app.shares.access(id, unix_ms(), false)
    } else {
        Access::Missing
    };
    if matches!(access, Access::Ok(_)) {
        return no_store(Response::new(204, "text/plain; charset=utf-8", Vec::new()));
    }
    if !matches!(access, Access::NeedsPassword) {
        let (status, body) = refusal(&access).unwrap_or((404, json!({"error": "missing"})));
        return no_store(Response::json(status, &body));
    }
    if let Some(refused) = unlock_gate(&app.shares, id, unix_ms()) {
        return refused;
    }
    let Ok(form) = req.json::<Unlock>() else {
        return no_store(Response::json(
            400,
            &json!({"error": "send the password as {\"password\": \"...\"}"}),
        ));
    };
    if !app.shares.check_password(id, &form.password) {
        app.shares.record(id, "unlock_failed", via(req));
        return no_store(Response::json(403, &json!({"error": "wrong password"})));
    }
    let Some(token) = app.shares.unlock_token(id, unix_ms()) else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    let mut cookie = format!(
        "apz_share_{id}={token}; Path=/artifact/{id}; HttpOnly; SameSite=Lax; Max-Age={}",
        UNLOCK_TTL_MS / 1000
    );
    if secure_context(app, req) {
        cookie.push_str("; Secure");
    }
    no_store(
        Response::new(204, "text/plain; charset=utf-8", Vec::new())
            .with_header("set-cookie", &cookie),
    )
}

/// `GET /artifact/{share}/blobs/{blob}`: a blob that the shared version's spec references.
fn blob_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let share = match admit(app, id, unlocked(app, req, id)) {
        Ok(share) => share,
        Err(refused) => return refused,
    };
    if share.target.kind != "artifact" {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    }
    let blob = p.get("blob");
    let Some(artifact) = app.artifacts.get(&share.target.id, share.version) else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    // The spec names its blobs by id; a blob the shared version does not name is missing to this link.
    let referenced =
        serde_json::to_string(&artifact).is_ok_and(|text| text.contains(&format!("\"{blob}\"")));
    let found = if referenced && safe_name(blob) == blob {
        app.artifacts.blob(blob)
    } else {
        None
    };
    let Some((content_type, bytes)) = found else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    let response =
        Response::new(200, &content_type, bytes).with_header("content-security-policy", SANDBOXED);
    finish(response, Some(&share))
}

/// `GET /artifact/{share}/rows?panel=&offset=&limit=&sort=&desc=&filter=`: one page of a shared dataset.
fn rows_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let share = match admit(app, id, unlocked(app, req, id)) {
        Ok(share) => share,
        Err(refused) => return refused,
    };
    if share.target.kind != "artifact" {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    }
    let panel = req.query_param("panel").unwrap_or("");
    let offset = number(req, "offset");
    let limit = number(req, "limit");
    let sort = req
        .query_param("sort")
        .filter(|key| !key.is_empty())
        .map(|key| (key, matches!(req.query_param("desc"), Some("1" | "true"))));
    let filter = req.query_param("filter");
    match app.artifacts.rows(
        &share.target.id,
        share.version,
        panel,
        offset,
        limit,
        sort,
        filter,
    ) {
        Ok(page) => finish(Response::json(200, &page), Some(&share)),
        Err(e) => no_store(Response::json(400, &json!({"error": e}))),
    }
}

/// `GET /artifact/{share}/export?format=csv|json&panel=`: the artifact as JSON, or a dataset panel as CSV. Refused
/// unless the share allows downloads. Records a download.
fn export_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let share = match admit(app, id, unlocked(app, req, id)) {
        Ok(share) => share,
        Err(refused) => return refused,
    };
    if share.target.kind != "artifact" {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    }
    if !share.allow_download {
        return no_store(Response::json(
            403,
            &json!({"error": "downloads are off for this share"}),
        ));
    }
    let Some(artifact) = app.artifacts.get(&share.target.id, share.version) else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    let stem = slug(&artifact.title);
    let (bytes, content_type, name) = match req.query_param("format").unwrap_or("json") {
        "json" => (
            serde_json::to_vec(&artifact).unwrap_or_default(),
            "application/json; charset=utf-8",
            format!("{stem}.json"),
        ),
        "csv" => {
            let Some(panel) = req.query_param("panel").filter(|s| !s.is_empty()) else {
                return no_store(Response::json(
                    400,
                    &json!({"error": "a CSV export needs the panel (?panel=)"}),
                ));
            };
            match crate::artifacts::routes::panel_table(app, &artifact, panel) {
                Ok((header, rows)) => (
                    crate::artifacts::routes::to_csv(&header, &rows).into_bytes(),
                    "text/csv; charset=utf-8",
                    format!("{stem}-{}.csv", safe_name(panel)),
                ),
                Err(e) => return no_store(Response::json(400, &json!({"error": e}))),
            }
        }
        "png" => {
            let Some(panel) = req.query_param("panel").filter(|s| !s.is_empty()) else {
                return no_store(Response::json(
                    400,
                    &json!({"error": "a PNG export needs the panel (?panel=)"}),
                ));
            };
            let dark = req.query_param("mode") == Some("dark");
            match crate::artifacts::routes::panel_png(app, &artifact, panel, dark) {
                Ok(bytes) => (
                    bytes,
                    "image/png",
                    format!("{stem}-{}.png", safe_name(panel)),
                ),
                Err(e) => return no_store(Response::json(400, &json!({"error": e}))),
            }
        }
        other => {
            return no_store(Response::json(
                400,
                &json!({"error": format!("format is csv, json or png, not {other:?}")}),
            ));
        }
    };
    app.shares.record(&share.id, "download", via(req));
    let response = attachment(Response::new(200, content_type, bytes), &name);
    finish(response, Some(&share))
}

/// `GET /artifact/{share}/file`: the bytes of a file share. Downloaded when the share allows it; otherwise shown
/// inline, and only for images and PDFs.
fn file_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let share = match admit(app, id, unlocked(app, req, id)) {
        Ok(share) => share,
        Err(refused) => return refused,
    };
    let item = match app.deliverables.find(&share.target.id) {
        Some(item) if share.target.kind == "file" => item,
        _ => return no_store(Response::json(404, &json!({"error": "missing"}))),
    };
    let inline = item.content_type.starts_with("image/") || item.content_type == "application/pdf";
    if !share.allow_download && !inline {
        return no_store(Response::json(
            403,
            &json!({"error": "downloads are off for this share"}),
        ));
    }
    let Ok(bytes) = app.deliverables.read_blob(&item) else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    let event = if share.allow_download {
        "download"
    } else {
        "view"
    };
    app.shares.record(&share.id, event, via(req));
    let mut response = Response::new(200, &item.content_type, bytes)
        .with_header("content-security-policy", SANDBOXED);
    if share.allow_download {
        response = attachment(response, &safe_name(&item.name));
    }
    finish(response, Some(&share))
}

/// `GET /artifact/{share}/frame/{panel}/{path..}`: a file of a panel's HTML, in a sandboxed frame. The path is the
/// generated page when empty.
fn frame_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("share");
    let share = match admit(app, id, unlocked(app, req, id)) {
        Ok(share) => share,
        Err(refused) => return refused,
    };
    let panel = p.get("panel");
    let path = p.get("path");
    if share.target.kind != "artifact" || safe_name(panel) != panel || !clean_path(path) {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    }
    let Some(artifact) = app.artifacts.get(&share.target.id, share.version) else {
        return no_store(Response::json(404, &json!({"error": "missing"})));
    };
    let origin = if via(req) == "relay" {
        format!("{RELAY_ORIGIN}/artifact/{id}/frame/{panel}")
    } else {
        format!("http://{}", host_of(req))
    };
    let response = put_header(
        content::serve_frame(app, &artifact, panel, path, &origin, FRAME_ANCESTORS),
        "content-security-policy",
        FRAME_SANDBOX,
    );
    finish(response, Some(&share))
}

/// The share `id` as a request may see it: the share, or the response that refuses it. A missing and a revoked
/// share answer alike, so a viewer cannot tell them apart.
fn admit(app: &App, id: &str, unlocked: bool) -> Result<Share, Response> {
    let access = if valid_id(id) {
        app.shares.access(id, unix_ms(), unlocked)
    } else {
        Access::Missing
    };
    if let Access::Ok(share) = &access {
        return Ok(share.clone());
    }
    let (status, body) = refusal(&access).unwrap_or((404, json!({"error": "missing"})));
    Err(no_store(Response::json(status, &body)))
}

/// The status and body that refuse an access, `None` when it is allowed. Missing and revoked shares answer the
/// same, so a viewer cannot tell a revoked link from one that never existed.
fn refusal(access: &Access) -> Option<(u16, Value)> {
    match access {
        Access::Ok(_) => None,
        Access::Missing | Access::Revoked => Some((404, json!({"error": "missing"}))),
        Access::Expired => Some((410, json!({"error": "expired"}))),
        Access::NeedsPassword => Some((401, json!({"needs_password": true}))),
    }
}

/// Whether the request carries the unlock cookie of share `id` with a valid, unexpired token for the current password.
fn unlocked(app: &App, req: &Request, id: &str) -> bool {
    let prefix = format!("apz_share_{id}=");
    req.header("cookie")
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&prefix))
        .is_some_and(|token| app.shares.token_ok(id, token, unix_ms()))
}

/// The `429` answer for a share whose failed unlocks have used up their window, or `None` when an attempt may run.
/// It is checked before the password is read, so a refused attempt costs no argon2 run.
fn unlock_gate(shares: &Store, id: &str, now_ms: u64) -> Option<Response> {
    if !shares.unlock_blocked(id, now_ms) {
        return None;
    }
    Some(no_store(Response::json(
        429,
        &json!({"error": "too many attempts; try again later"}),
    )))
}

/// Whether a response for `share` may be cached: a live share that pins its version, has no password and no expiry.
fn cacheable(share: &Share) -> bool {
    share.version.is_some()
        && share.password_hash.is_none()
        && share.expires_ms.is_none()
        && share.revoked_ms.is_none()
}

/// Sets header `name` to `value`, replacing any header of that name already on the response. Unlike `with_header`,
/// which appends, a response never carries two `Cache-Control` or two `Content-Security-Policy` headers.
fn put_header(mut response: Response, name: &str, value: &str) -> Response {
    response
        .headers
        .retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
    response.headers.push((name.to_string(), value.to_string()));
    response
}

/// Adds the headers every public response carries, with the given `Cache-Control`. Each header is set once.
fn secure(response: Response, cache: &str) -> Response {
    let response = put_header(response, "x-robots-tag", "noindex");
    let response = put_header(response, "referrer-policy", "no-referrer");
    let response = put_header(response, "x-content-type-options", "nosniff");
    put_header(response, "cache-control", cache)
}

/// [`secure`] with `no-store`, for every answer that is not a cacheable share.
fn no_store(response: Response) -> Response {
    secure(response, "no-store")
}

/// Finishes a response for `share`: cached when the share is cacheable, `no-store` otherwise.
fn finish(response: Response, share: Option<&Share>) -> Response {
    match share.filter(|s| cacheable(s)) {
        Some(_) => put_header(
            secure(response, "public, max-age=60"),
            "x-apz-cache",
            "public",
        ),
        None => no_store(response),
    }
}

/// The share id as the store issues it: 32 lowercase hex characters.
fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Whether a request came through the relay or the hub's own network link. The relay marks each request it forwards
/// with `x-apz-relay`, a secret per process (see `relay::relay_marker`). A client's own `x-apz-via` or
/// `x-apz-relay` header proves nothing, so only the marker counts.
fn via(req: &Request) -> &'static str {
    if req.header("x-apz-relay") == Some(relay::relay_marker()) {
        "relay"
    } else {
        "network"
    }
}

/// Whether a cookie for this request gets `Secure`: a relayed request (the viewer reached agentplaza.io over HTTPS)
/// or any request to a hub that serves HTTPS itself (`--tls`, or any bind off loopback). An `x-forwarded-proto` header
/// is not trusted, because any network client can send it.
fn secure_context(app: &App, req: &Request) -> bool {
    secure_for(via(req), app.config.tls, app.config.bind.is_loopback())
}

/// [`secure_context`]'s rule on its inputs.
fn secure_for(via: &str, tls: bool, loopback: bool) -> bool {
    via == "relay" || tls || !loopback
}

/// The `Host` of the request, limited to the characters a host name can hold.
fn host_of(req: &Request) -> String {
    req.header("host")
        .filter(|h| {
            h.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".:-[]".contains(&c))
        })
        .unwrap_or("localhost")
        .to_string()
}

/// The origin the viewer's page is served from: the relay's, or this hub's `Host`.
fn content_origin(req: &Request) -> String {
    if via(req) == "relay" {
        RELAY_ORIGIN.to_string()
    } else {
        format!("http://{}", host_of(req))
    }
}

/// The viewer's content security policy, with the frames allowed from `frame_origin`.
fn viewer_csp(frame_origin: &str) -> String {
    format!(
        "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
         font-src 'self'; connect-src 'self' {RELAY_ORIGIN}; frame-src 'self' {frame_origin}; base-uri 'none'; \
         form-action 'none'; frame-ancestors 'none'"
    )
}

/// The share's fields the viewer shows. `latest` is `None` for a file.
fn share_value(share: &Share, latest: Option<u32>) -> Value {
    json!({
        "id": share.id,
        "title": share.title,
        "kind": share.target.kind,
        "allow_download": share.allow_download,
        "version": share.version,
        "latest": latest,
    })
}

/// The owner's theme for the viewer: the palette and skin the choice resolves to, the mode a viewer sees (System is
/// light), and the CSS of a saved theme for both modes.
fn theme_value(app: &App) -> Value {
    let doc = app.appearance.get();
    let mut light = doc.choice.clone();
    light.mode = ModeChoice::Light;
    let mut dark = doc.choice.clone();
    dark.mode = ModeChoice::Dark;
    let light_resolved = light.resolve(&doc.themes, true);
    let dark_resolved = dark.resolve(&doc.themes, false);
    let mode = if doc.choice.mode == ModeChoice::Dark {
        "dark"
    } else {
        "light"
    };
    let css = if builtin::is_builtin(&doc.choice.theme) {
        None
    } else {
        Some(format!(
            "{}{}",
            theme_css(":root[data-theme=\"light\"]", &light_resolved),
            theme_css(":root[data-theme=\"dark\"]", &dark_resolved)
        ))
    };
    json!({
        "skin": light_resolved.skin.id,
        "palette": doc.choice.theme,
        "mode": mode,
        "css": css,
    })
}

/// Sets `Content-Disposition: attachment` with a sanitised file name.
fn attachment(response: Response, name: &str) -> Response {
    response.with_header(
        "content-disposition",
        &format!("attachment; filename=\"{name}\""),
    )
}

/// The content type of a viewer asset, from its extension.
fn asset_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "json" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// A file name made of `[A-Za-z0-9._-]`: other characters become dashes, leading dots are dropped and `..` is never
/// left. `download` when nothing is left.
fn safe_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.').replace("..", "-");
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed
    }
}

/// A file stem from a title: lower-case ASCII words joined by dashes, `artifact` when nothing is left.
fn slug(title: &str) -> String {
    let lowered: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let words: Vec<&str> = lowered.split('-').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        "artifact".to_string()
    } else {
        words.join("-")
    }
}

/// Whether a frame path is safe to hand on: empty (the generated page), or segments that are neither empty, `.`,
/// `..` nor carry a backslash or a NUL.
fn clean_path(path: &str) -> bool {
    path.is_empty()
        || path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(['\\', '\0']))
}

/// A numeric query parameter, 0 when it is missing or not a number.
fn number(req: &Request, key: &str) -> u64 {
    req.query_param(key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::{Options, Store, Target, UNLOCK_LIMIT};

    fn artifact(id: &str) -> Target {
        Target {
            kind: "artifact".into(),
            id: id.into(),
        }
    }

    #[test]
    fn a_missing_share_answers_404() {
        let store = Store::for_tests();
        let (status, body) = refusal(&store.access(&"f".repeat(32), unix_ms(), false))
            .expect("a missing share is refused");
        assert_eq!(status, 404);
        assert_eq!(body, json!({"error": "missing"}));
    }

    #[test]
    fn a_revoked_share_is_indistinguishable_from_a_missing_one() {
        let store = Store::for_tests();
        let share = store
            .create(artifact("art-1"), "A".into(), Options::default(), "cid-1")
            .expect("the share is saved");
        store.revoke(&share.id).expect("the save succeeds");
        assert_eq!(
            refusal(&store.access(&share.id, unix_ms(), false)),
            refusal(&store.access(&"f".repeat(32), unix_ms(), false)),
        );
    }

    #[test]
    fn an_expired_share_answers_410() {
        let store = Store::for_tests();
        let opts = Options {
            expires_ms: Some(1_000),
            ..Options::default()
        };
        let share = store
            .create(artifact("art-1"), "A".into(), opts, "cid-1")
            .expect("the share is saved");
        assert_eq!(
            refusal(&store.access(&share.id, 1_000, false)),
            Some((410, json!({"error": "expired"}))),
        );
    }

    #[test]
    fn a_password_share_needs_the_unlock_cookie() {
        let store = Store::for_tests();
        let opts = Options {
            password: Some("pw".into()),
            ..Options::default()
        };
        let share = store
            .create(artifact("art-1"), "A".into(), opts, "cid-1")
            .expect("the share is saved");
        let (status, body) = refusal(&store.access(&share.id, unix_ms(), false))
            .expect("a password share is refused without the cookie");
        assert_eq!(status, 401);
        assert_eq!(body, json!({"needs_password": true}));

        let now = unix_ms();
        let token = store
            .unlock_token(&share.id, now)
            .expect("a token for a password share");
        assert!(
            refusal(&store.access(&share.id, now, store.token_ok(&share.id, &token, now)))
                .is_none()
        );
        assert!(
            refusal(&store.access(&share.id, now, store.token_ok(&share.id, "00", now))).is_some()
        );
    }

    #[test]
    fn the_frame_policy_sandboxes_and_never_allows_same_origin() {
        assert!(FRAME_SANDBOX.starts_with("sandbox"));
        assert!(!FRAME_SANDBOX.contains("allow-same-origin"));
    }

    #[test]
    fn share_paths_pass_the_owner_gate_and_v1_does_not() {
        assert!(crate::hub_auth::is_public("GET", "/artifact/x/data"));
        assert!(!crate::hub_auth::is_public("GET", "/v1/settings"));
    }

    #[test]
    fn a_traversing_path_is_refused_before_it_reaches_a_handler() {
        assert!(!valid_id("x/../v1/settings"));
        assert!(!clean_path("../v1/settings"));
        assert!(!clean_path("a/../../v1/settings"));
        assert!(!clean_path("a//b"));
        assert!(clean_path(""));
        assert!(clean_path("assets/app.js"));
    }

    #[test]
    fn share_ids_are_32_lowercase_hex_characters() {
        assert!(valid_id(&"a1".repeat(16)));
        assert!(!valid_id("x"));
        assert!(!valid_id(&"A1".repeat(16)));
    }

    #[test]
    fn only_a_pinned_open_share_is_cacheable() {
        let store = Store::for_tests();
        let pinned = store
            .create(
                artifact("art-1"),
                "A".into(),
                Options {
                    version: Some(1),
                    ..Options::default()
                },
                "cid-1",
            )
            .expect("the share is saved");
        assert!(cacheable(&pinned));
        let latest = store
            .create(artifact("art-2"), "B".into(), Options::default(), "cid-1")
            .expect("the share is saved");
        assert!(
            !cacheable(&latest),
            "a share that follows the latest version is not cached"
        );
        let locked = store
            .create(
                artifact("art-3"),
                "C".into(),
                Options {
                    version: Some(1),
                    password: Some("pw".into()),
                    ..Options::default()
                },
                "cid-1",
            )
            .expect("the share is saved");
        assert!(!cacheable(&locked));
    }

    #[test]
    fn download_names_keep_only_safe_characters() {
        assert_eq!(safe_name("../../etc/passwd"), "---etc-passwd");
        assert_eq!(
            safe_name("report 2026 (final).csv"),
            "report-2026--final-.csv"
        );
        assert_eq!(safe_name("..."), "download");
        assert_eq!(safe_name(""), "download");
        assert_eq!(slug("Q3 Revenue: by region"), "q3-revenue-by-region");
    }

    /// A request for the share path with the given headers, for the relay and cache tests.
    fn request_with(headers: &[(&str, &str)]) -> Request {
        Request {
            method: "GET".into(),
            path: "/artifact/x".into(),
            query: String::new(),
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: Vec::new(),
            peer: None,
        }
    }

    #[test]
    fn the_eleventh_failed_unlock_gets_429() {
        let store = Store::for_tests();
        let opts = Options {
            password: Some("a-long-password".into()),
            ..Options::default()
        };
        let share = store
            .create(artifact("art-1"), "A".into(), opts, "cid-1")
            .expect("the share is saved");
        let now = unix_ms();
        for _ in 0..UNLOCK_LIMIT {
            assert!(unlock_gate(&store, &share.id, now).is_none());
            store.record(&share.id, "unlock_failed", "network");
        }
        let refused = unlock_gate(&store, &share.id, now).expect("the 11th attempt is refused");
        assert_eq!(refused.status, 429);
        let body: Value = serde_json::from_slice(refused.bytes()).expect("a JSON body");
        assert_eq!(body, json!({"error": "too many attempts; try again later"}));
    }

    #[test]
    fn a_spoofed_relay_header_is_not_a_relayed_request() {
        assert_eq!(via(&request_with(&[("x-apz-via", "relay")])), "network");
        assert_eq!(via(&request_with(&[("x-apz-relay", "guessed")])), "network");
        let marked = [("x-apz-relay", relay::relay_marker())];
        assert_eq!(via(&request_with(&marked)), "relay");
        assert!(!secure_for(
            via(&request_with(&[("x-forwarded-proto", "https")])),
            false,
            true
        ));
        assert!(secure_for(via(&request_with(&marked)), false, true));
        assert!(
            secure_for("network", true, true),
            "a hub serving HTTPS sets Secure"
        );
    }

    #[test]
    fn a_cacheable_share_has_one_short_cache_header() {
        let store = Store::for_tests();
        let opts = Options {
            version: Some(1),
            ..Options::default()
        };
        let share = store
            .create(artifact("art-1"), "A".into(), opts, "cid-1")
            .expect("the share is saved");
        let response = finish(Response::json(200, &json!({})), Some(&share));
        let caches: Vec<&str> = response
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("cache-control"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(caches, vec!["public, max-age=60"]);
    }
}
