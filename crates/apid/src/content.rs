//! The content origin (docs/design/artifacts.md, "HTML artifacts"): a second port, apart from the console's
//! origin, that serves HTML artifact frames under one CSP with the `sandbox` directive, and the vendored `/_lib/`
//! set.
//!
//! A frame's URL carries its own capability, `/a/{artifact}/{version}/{panel}/{cap}/{path}`, where `cap` is an
//! expiring HMAC over the artifact, version and panel. The page therefore needs no owner token, and a frame
//! cannot reach the hub's API from its origin. The owner asks for a frame's URL with `GET /v1/artifacts/{id}/frame`.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_view::artifact::{Artifact, is_script, safe_path};
use apz_view::libs::{LIBS, by_file};
use apz_view::{Body, HtmlBody, HtmlFile};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::state::App;

/// The bridge script every frame loads first: it reports the page's height and carries links and downloads out.
const BRIDGE: &str = include_str!("content_bridge.js");

/// How long a frame's capability lasts: an hour. The console asks for a fresh URL each time an artifact opens.
const FRAME_TTL_MS: u64 = 60 * 60 * 1000;

/// The browser cache policy of a vendored library: its file name never changes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The extensions a path without one may resolve to, in order.
const SCRIPT_EXTS: [&str; 5] = [".tsx", ".ts", ".jsx", ".mjs", ".js"];

/// The port from `--content-port`, recorded by [`take_port`]. Unset, the content port is the API port plus one.
static CONTENT_PORT: OnceLock<u16> = OnceLock::new();

/// The frame secret, read once from `<state-dir>/content_secret`.
static SECRET: OnceLock<Vec<u8>> = OnceLock::new();

/// Takes `--content-port PORT` out of the arguments, records the port for [`spawn`], and returns the rest in order.
pub fn take_port(args: Vec<String>) -> Result<Vec<String>, String> {
    let mut rest = Vec::with_capacity(args.len());
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--content-port" {
            let value = args
                .next()
                .ok_or_else(|| "--content-port needs a port".to_string())?;
            let port = value
                .parse::<u16>()
                .map_err(|err| format!("--content-port: {err}"))?;
            let _ = CONTENT_PORT.set(port);
        } else {
            rest.push(arg);
        }
    }
    Ok(rest)
}

/// The owner's route for opening a frame: `GET /v1/artifacts/{id}/frame`.
pub fn routes(router: Router<App>) -> Router<App> {
    router.get("/v1/artifacts/{id}/frame", frame_route).doc(
        RouteDoc::new("The content-origin URL of an HTML panel (?v=&panel=), valid for an hour")
            .errors(&[400, 404]),
    )
}

/// Starts the content listener beside the API: the same address and TLS setup, on the content port. Its handle
/// is not needed to keep it running.
pub fn spawn(app: &App) -> io::Result<apz_http::Handle> {
    let config = &app.config;
    let port = content_port(app)?;
    let mut server = apz_http::Server::bind_addr(SocketAddr::new(config.bind, port))?;
    if config.tls || !config.bind.is_loopback() {
        let dir = config.state_dir.join("tls");
        let pair = apz_trust::tls::CertPair {
            cert_pem: std::fs::read_to_string(dir.join("cert.pem"))?,
            key_pem: std::fs::read_to_string(dir.join("key.pem"))?,
        };
        let tls = apz_trust::tls::server_config(&pair).map_err(io::Error::other)?;
        server = server.tls(tls);
    }
    let app = app.handle();
    server.spawn(move |request| match app.upgrade() {
        Some(app) => serve(&app, request),
        None => Response::error(503, "the hub is stopping"),
    })
}

/// The content port: the flag's, or the API port plus one.
fn content_port(app: &App) -> io::Result<u16> {
    match CONTENT_PORT.get() {
        Some(port) => Ok(*port),
        None => app.config.port.checked_add(1).ok_or_else(|| {
            io::Error::other("the API port is 65535, so the content port needs --content-port")
        }),
    }
}

/// Answers one request on the content listener.
fn serve(app: &App, req: &Request) -> Response {
    let host = match hub_host(app, req) {
        Ok(host) => host,
        Err(response) => return response,
    };
    let origin = format!("http://{host}");
    let ancestors = frame_ancestors(app, host);
    let path = req.path.as_str();
    if let Some(rest) = path.strip_prefix("/a/") {
        let mut response = frame_request(app, rest, &origin, &ancestors);
        put(&mut response, &frame_headers(&origin, &ancestors));
        return response;
    }
    let mut response = if let Some(file) = path.strip_prefix("/_lib/") {
        lib_response(app, file)
    } else if path == "/_bridge.js" {
        Response::new(200, "text/javascript; charset=utf-8", BRIDGE)
    } else {
        Response::error(404, "not found")
    };
    put(&mut response, &frame_headers(&origin, &ancestors));
    if response.status == 200 && path.starts_with("/_lib/") {
        put(&mut response, &[("cache-control".into(), IMMUTABLE.into())]);
    }
    response
}

/// `/a/{artifact}/{version}/{panel}/{cap}/{path}`: a frame's file, once its capability checks out.
fn frame_request(app: &App, rest: &str, origin: &str, ancestors: &str) -> Response {
    let mut parts = rest.splitn(5, '/');
    let (Some(id), Some(version), Some(panel), Some(cap)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Response::error(404, "not found");
    };
    let path = parts.next().unwrap_or("");
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(404, "not found");
    };
    let Ok(secret) = frame_secret(app) else {
        return Response::error(500, "the frame secret is unreadable");
    };
    if !cap_valid(&secret, id, version, panel, cap, now_ms()) {
        return Response::error(403, "the frame's link has expired or is not valid");
    }
    let Some(artifact) = app.artifacts.get(id, Some(version)) else {
        return Response::error(404, "not found");
    };
    serve_frame(app, &artifact, panel, path, origin, ancestors)
}

/// One file of an HTML artifact's panel, for a frame at `origin` (the content origin) that `frame_ancestors`
/// may embed. An empty path, or `index.html`, of a component entry (`.jsx`, `.tsx`) is the generated page.
pub fn serve_frame(
    app: &App,
    artifact: &Artifact,
    panel: &str,
    path: &str,
    origin: &str,
    frame_ancestors: &str,
) -> Response {
    let mut response = frame_body(app, artifact, panel, path);
    put(&mut response, &frame_headers(origin, frame_ancestors));
    response
}

/// The body of [`serve_frame`], before the headers are set.
fn frame_body(app: &App, artifact: &Artifact, panel: &str, path: &str) -> Response {
    let Some(html) = html_panel(artifact, panel) else {
        return Response::error(404, "not found");
    };
    let component = matches!(extension(&html.entry), Some("jsx" | "tsx"));
    if component && (path.is_empty() || path == "index.html") {
        if !safe_path(&html.entry) {
            return Response::error(404, "not found");
        }
        return Response::new(200, "text/html; charset=utf-8", component_page(html));
    }
    // An empty path, or `index.html` when the page has no file of that name, is the entry: share frames are opened at
    // `…/frame/{panel}/index.html` so relative paths inside the page resolve under the panel.
    let entry =
        path.is_empty() || (path == "index.html" && find_file(html, "index.html").is_none());
    let wanted = if entry { html.entry.as_str() } else { path };
    let Some(file) = find_file(html, wanted) else {
        return Response::error(404, "not found");
    };
    if let Some(text) = &file.text {
        if file.path.ends_with(".html") {
            return Response::new(200, content_type(&file.path), with_bridge(text));
        }
        if is_script(&file.path) {
            let js = compiled(app, &artifact.id, artifact.version, panel, &file.path)
                .unwrap_or_else(|| text.clone());
            return Response::new(200, content_type(&file.path), js);
        }
        return Response::new(200, content_type(&file.path), text.clone());
    }
    match file
        .blob
        .as_ref()
        .and_then(|blob| app.artifacts.artifact_blob(&artifact.id, &blob.id))
    {
        Some((content_type, bytes)) => Response::new(200, &content_type, bytes),
        None => Response::error(404, "not found"),
    }
}

/// The hub's compiled JavaScript for a script of panel `panel` of the artifact (TypeScript and JSX compiled with oxc on
/// create), at the version the page was made from.
fn compiled(app: &App, id: &str, version: u32, panel: &str, path: &str) -> Option<String> {
    app.artifacts.compiled(id, version, panel, path)
}

/// One vendored library from the console's `_lib` directory, when the library set names it.
fn lib_response(app: &App, file: &str) -> Response {
    if by_file(file).is_none() {
        return Response::error(404, "not found");
    }
    match std::fs::read(app.config.console.join("_lib").join(file)) {
        Ok(bytes) => Response::new(200, content_type(file), bytes),
        Err(_) => Response::error(404, "not found"),
    }
}

/// The owner's frame route: `GET /v1/artifacts/{id}/frame?v=&panel=`, the content-origin URL of one HTML panel.
fn frame_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let version = match req.query_param("v") {
        None => None,
        Some(v) => match v.parse::<u32>() {
            Ok(v) => Some(v),
            Err(_) => return Response::error(400, "v must be a version number"),
        },
    };
    let Some(artifact) = app.artifacts.get(p.get("id"), version) else {
        return Response::error(404, "no such artifact");
    };
    let panel = req.query_param("panel").unwrap_or("");
    if html_panel(&artifact, panel).is_none() {
        return Response::error(404, "no such HTML panel");
    }
    let host = match hub_host(app, req) {
        Ok(host) => host,
        Err(response) => return response,
    };
    let secret = match frame_secret(app) {
        Ok(secret) => secret,
        Err(_) => return Response::error(500, "the frame secret is unreadable"),
    };
    let Some(capability) = cap(
        &secret,
        &artifact.id,
        artifact.version,
        panel,
        now_ms() + FRAME_TTL_MS,
    ) else {
        return Response::error(500, "the frame capability could not be made");
    };
    let port = match content_port(app) {
        Ok(port) => port,
        Err(err) => return Response::error(500, &err.to_string()),
    };
    let scheme = if app.config.tls { "https" } else { "http" };
    let url = format!(
        "{scheme}://{}:{port}/a/{}/{}/{panel}/{capability}/",
        host_only(host),
        artifact.id,
        artifact.version
    );
    Response::json(200, &json!({ "url": url }))
}

/// The content origin the owner's console opens frames from, as the frame route builds it for this request's host;
/// `None` when the request names no host this hub answers to.
pub fn frame_origin(app: &App, req: &Request) -> Option<String> {
    let host = hub_host(app, req).ok()?;
    let port = content_port(app).ok()?;
    let scheme = if app.config.tls { "https" } else { "http" };
    Some(format!("{scheme}://{}:{port}", host_only(host)))
}

/// The console's start page with the content origin added to its policy's `frame-src`, so its HTML panels load: the
/// page's own `<meta>` policy names only `'self'`, and the content listener is another port.
pub fn console_page(page: &str, frame_origin: Option<&str>) -> String {
    match frame_origin {
        Some(origin) => page.replacen("frame-src 'self'", &format!("frame-src 'self' {origin}"), 1),
        None => page.to_string(),
    }
}

/// The frame secret: read from `<state-dir>/content_secret`, or made (32 random bytes, owner-only) on first use.
fn frame_secret(app: &App) -> io::Result<Vec<u8>> {
    if let Some(secret) = SECRET.get() {
        return Ok(secret.clone());
    }
    let path = app.config.state_dir.join("content_secret");
    let secret = match std::fs::read(&path) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => {
            let bytes = apz_trust::random::<32>().to_vec();
            write_private(&path, &bytes)?;
            bytes
        }
    };
    let _ = SECRET.set(secret.clone());
    Ok(secret)
}

/// Writes a file that only the owner can read.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// A frame's capability for one panel of one version: `{exp_hex}.{mac_hex}`, where the MAC is an HMAC-SHA256 over
/// `{artifact}/{version}/{panel}/{exp_hex}`.
pub fn cap(
    secret: &[u8],
    artifact: &str,
    version: u32,
    panel: &str,
    exp_ms: u64,
) -> Option<String> {
    let exp = format!("{exp_ms:x}");
    let tag = tag(secret, artifact, version, panel, &exp)?;
    Some(format!("{exp}.{}", hex(&tag)))
}

/// Whether a capability is well formed, unexpired at `now_ms`, and signed for this artifact, version and panel.
pub fn cap_valid(
    secret: &[u8],
    artifact: &str,
    version: u32,
    panel: &str,
    cap: &str,
    now_ms: u64,
) -> bool {
    let Some((exp, tag_hex)) = cap.split_once('.') else {
        return false;
    };
    if exp.is_empty() || exp.len() > 16 || !exp.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let Ok(exp_ms) = u64::from_str_radix(exp, 16) else {
        return false;
    };
    if now_ms >= exp_ms {
        return false;
    }
    let (Some(want), Some(got)) = (tag(secret, artifact, version, panel, exp), unhex(tag_hex))
    else {
        return false;
    };
    apz_trust::ct_eq(&want, &got)
}

/// The HMAC-SHA256 tag over one capability's fields.
fn tag(secret: &[u8], artifact: &str, version: u32, panel: &str, exp: &str) -> Option<Vec<u8>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).ok()?;
    mac.update(format!("{artifact}/{version}/{panel}/{exp}").as_bytes());
    Some(mac.finalize().into_bytes().to_vec())
}

/// The headers every frame response carries: one CSP with the `sandbox` directive (no `allow-same-origin`), a
/// locked-down permissions policy, and no caching. `origin` is the content origin, and `frame_ancestors` lists
/// the consoles that may embed the frame.
pub fn frame_headers(origin: &str, frame_ancestors: &str) -> Vec<(String, String)> {
    let csp = format!(
        "sandbox allow-scripts allow-popups allow-downloads; default-src 'none'; \
         script-src 'self' 'unsafe-inline' blob: {origin}; style-src 'self' 'unsafe-inline' {origin}; \
         img-src 'self' data: blob: {origin}; font-src 'self' data: {origin}; connect-src 'self' {origin}; \
         media-src 'self' data: blob:; form-action 'none'; object-src 'none'; base-uri 'none'; \
         frame-ancestors {frame_ancestors}"
    );
    [
        ("Content-Security-Policy", csp.as_str()),
        (
            "Permissions-Policy",
            "camera=(), microphone=(), geolocation=(), clipboard-read=()",
        ),
        ("Referrer-Policy", "no-referrer"),
        ("X-Content-Type-Options", "nosniff"),
        ("Access-Control-Allow-Origin", "*"),
        ("Cross-Origin-Resource-Policy", "cross-origin"),
        ("Cache-Control", "no-store"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

/// The consoles that may embed a frame: the API's loopback names, the hosted console, and (under TLS) the hub's
/// own host name.
fn frame_ancestors(app: &App, host: &str) -> String {
    let port = app.config.port;
    let mut out = format!("http://127.0.0.1:{port} http://localhost:{port} https://agentplaza.io");
    if app.config.tls {
        out.push_str(&format!(" https://{}:{port}", host_only(host)));
    }
    out
}

/// The request's Host header when it names this hub. A missing or unusable header is a 400; any other name is a 421,
/// so a frame is never served under a name that is not the hub's own.
fn hub_host<'a>(app: &App, req: &'a Request) -> Result<&'a str, Response> {
    let Some(host) = host_of(req) else {
        return Err(Response::error(400, "no usable Host header"));
    };
    let public = public_name(app);
    if name_allowed(host_only(host), app.config.bind, &public) {
        Ok(host)
    } else {
        Err(Response::error(421, "the Host header names another server"))
    }
}

/// The host name of the configured public URL, without its scheme, port or path; empty when there is none.
fn public_name(app: &App) -> String {
    let url = app.config.public_url.as_str();
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    host_only(after_scheme.split('/').next().unwrap_or("")).to_owned()
}

/// Whether a host name (no port) is one this hub answers to: a loopback name, the configured bind address, or the
/// host of the public URL. Brackets around an IPv6 address are ignored.
fn name_allowed(name: &str, bind: std::net::IpAddr, public: &str) -> bool {
    let name = name.trim_start_matches('[').trim_end_matches(']');
    let public = public.trim_start_matches('[').trim_end_matches(']');
    if name.is_empty() {
        return false;
    }
    let loopback = name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "::1";
    let bound = name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip == bind);
    loopback || bound || name.eq_ignore_ascii_case(public)
}

/// The request's Host header, when it holds only characters a host name or an address can have.
fn host_of(req: &Request) -> Option<&str> {
    let host = req.header("host")?;
    let plain = !host.is_empty()
        && host.len() <= 260
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    plain.then_some(host)
}

/// A host without its port: `example.test` for `example.test:7700`, `[::1]` for `[::1]:7700`.
fn host_only(host: &str) -> &str {
    if host.starts_with('[') {
        host.find(']').map_or(host, |end| &host[..=end])
    } else {
        host.split(':').next().unwrap_or(host)
    }
}

/// The HTML panel with this id, when the artifact has one.
fn html_panel<'a>(artifact: &'a Artifact, panel: &str) -> Option<&'a HtmlBody> {
    match &artifact.panels.iter().find(|p| p.id == panel)?.body {
        Body::Html(html) => Some(html),
        _ => None,
    }
}

/// The file a request names: the exact path, or for a path without an extension the script of that name.
fn find_file<'a>(html: &'a HtmlBody, wanted: &str) -> Option<&'a HtmlFile> {
    if !safe_path(wanted) {
        return None;
    }
    if let Some(file) = html.files.iter().find(|f| f.path == wanted) {
        return Some(file);
    }
    if wanted
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.'))
    {
        return None;
    }
    SCRIPT_EXTS.iter().find_map(|ext| {
        let named = format!("{wanted}{ext}");
        html.files.iter().find(|f| f.path == named)
    })
}

/// The page a component entry runs in: the bridge, the libraries the artifact uses in load order, and the
/// component rendered into `#root`.
fn component_page(html: &HtmlBody) -> String {
    let libs: String = html
        .libs
        .iter()
        .filter_map(|name| LIBS.iter().find(|lib| lib.name == name.as_str()))
        .map(|lib| format!("<script src=\"/_lib/{}\"></script>\n", lib.file))
        .collect();
    format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <script src=\"/_bridge.js\"></script>\n{libs}\
         <style>html,body{{margin:0;background:var(--sheet,#fff);color:var(--ink,#111);\
         font:13.5px/1.45 Geist,system-ui,sans-serif}}</style>\n</head>\n<body>\n\
         <div id=\"root\"></div>\n\
         <script type=\"module\">import App from './{entry}'; \
         ReactDOM.createRoot(document.getElementById('root')).render(React.createElement(App));</script>\n\
         </body>\n</html>\n",
        entry = html.entry
    )
}

/// An HTML page with the bridge script first in its head, or at its start when it has no head.
fn with_bridge(html: &str) -> String {
    let tag = "<script src=\"/_bridge.js\"></script>";
    let lower = html.to_ascii_lowercase();
    let head = lower.match_indices("<head").map(|(at, _)| at).find(|&at| {
        matches!(
            lower.as_bytes().get(at + 5),
            Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')
        )
    });
    if let Some(start) = head
        && let Some(close) = html[start..].find('>')
    {
        let at = start + close + 1;
        return format!("{}{tag}{}", &html[..at], &html[at..]);
    }
    format!("{tag}{html}")
}

/// Sets a header, replacing any of the same name.
fn put(response: &mut Response, headers: &[(String, String)]) {
    for (name, value) in headers {
        response
            .headers
            .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
        response.headers.push((name.clone(), value.clone()));
    }
}

/// The extension of a path's last segment, without the dot.
fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    let (_, ext) = name.rsplit_once('.')?;
    Some(ext)
}

/// The media type a served file gets, from its extension.
fn content_type(path: &str) -> &'static str {
    match extension(path).map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "jsx" | "ts" | "tsx") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("csv") => "text/csv; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn html(entry: &str, files: &[(&str, &str)], libs: &[&str]) -> HtmlBody {
        HtmlBody {
            entry: entry.into(),
            files: files
                .iter()
                .map(|(path, text)| HtmlFile {
                    path: (*path).into(),
                    text: Some((*text).into()),
                    blob: None,
                })
                .collect(),
            height: 360,
            libs: libs.iter().map(|lib| (*lib).into()).collect(),
            still: None,
        }
    }

    #[test]
    fn the_console_page_may_frame_the_content_origin() {
        let page = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; frame-src 'self'; font-src 'self'">"#;
        let out = console_page(page, Some("http://127.0.0.1:7701"));
        assert!(out.contains("frame-src 'self' http://127.0.0.1:7701;"));
        assert_eq!(console_page(page, None), page);
    }

    #[test]
    fn a_foreign_host_is_refused() {
        let bind: std::net::IpAddr = "0.0.0.0".parse().unwrap();
        assert!(name_allowed("127.0.0.1", bind, "hub.example"));
        assert!(name_allowed("localhost", bind, ""));
        assert!(name_allowed("[::1]", bind, ""));
        assert!(name_allowed("hub.example", bind, "hub.example"));
        assert!(name_allowed(
            "192.168.1.5",
            "192.168.1.5".parse().unwrap(),
            ""
        ));
        assert!(!name_allowed("evil.example", bind, "hub.example"));
        assert!(!name_allowed("127.0.0.2.nip.io", bind, ""));
        assert!(!name_allowed("192.168.1.5", bind, ""));
        assert!(!name_allowed("", bind, ""));
    }

    #[test]
    fn capabilities_round_trip_and_expire() {
        let now = 1_000_000;
        let token = cap(SECRET, "art-1", 2, "ttft", now + 5_000).unwrap();
        assert!(cap_valid(SECRET, "art-1", 2, "ttft", &token, now));
        assert!(!cap_valid(SECRET, "art-1", 2, "ttft", &token, now + 5_000));
        assert!(!cap_valid(SECRET, "art-1", 3, "ttft", &token, now));
        assert!(!cap_valid(SECRET, "art-1", 2, "other", &token, now));
        assert!(!cap_valid(
            b"another secret of some length",
            "art-1",
            2,
            "ttft",
            &token,
            now
        ));
        assert!(!cap_valid(SECRET, "art-1", 2, "ttft", "zz.00", now));
        assert!(!cap_valid(SECRET, "art-1", 2, "ttft", "nodot", now));
    }

    #[test]
    fn frames_are_sandboxed_without_same_origin() {
        let headers = frame_headers("http://127.0.0.1:7701", "http://127.0.0.1:7700");
        let csp = headers
            .iter()
            .find(|(name, _)| name == "Content-Security-Policy")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(csp.contains("sandbox allow-scripts"));
        assert!(!csp.contains("allow-same-origin"));
        assert!(csp.contains("frame-ancestors http://127.0.0.1:7700"));
    }

    #[test]
    fn a_component_page_loads_its_libraries_in_order() {
        let names: Vec<&str> = LIBS.iter().take(2).map(|lib| lib.name).collect();
        let body = html(
            "App.tsx",
            &[("App.tsx", "export default function App() {}")],
            &names,
        );
        let page = component_page(&body);
        let first = page.find(&format!("/_lib/{}", LIBS[0].file)).unwrap();
        let second = page.find(&format!("/_lib/{}", LIBS[1].file)).unwrap();
        assert!(first < second);
        assert!(page.find("/_bridge.js").unwrap() < first);
        assert!(page.contains("import App from './App.tsx'"));
    }

    #[test]
    fn the_bridge_goes_first_in_an_html_head() {
        let page = with_bridge("<!doctype html><html><head><title>x</title></head></html>");
        let bridge = page.find("/_bridge.js").unwrap();
        assert!(page.find("<head>").unwrap() < bridge);
        assert!(bridge < page.find("<title>").unwrap());
        assert!(with_bridge("<p>hi</p>").starts_with("<script src=\"/_bridge.js\">"));
    }

    #[test]
    fn unsafe_and_unknown_paths_are_refused() {
        let body = html(
            "index.html",
            &[("index.html", "<p>x</p>"), ("lib/util.js", "export {}")],
            &[],
        );
        assert!(find_file(&body, "../secret.js").is_none());
        assert!(find_file(&body, "/etc/passwd").is_none());
        assert!(find_file(&body, "_lib/react.js").is_none());
        assert!(find_file(&body, "a\\b.js").is_none());
        assert!(find_file(&body, "missing.js").is_none());
        assert!(find_file(&body, "index.html").is_some());
        assert!(find_file(&body, "lib/util").is_some());
    }
}
