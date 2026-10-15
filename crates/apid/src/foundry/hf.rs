//! The Hugging Face client (`docs/design/foundry.md`, "Hugging Face"): the search and detail mapping,
//! the file tree, the importable hints, the egress rules and the HTTP calls. The parsers are pure and
//! tested on recorded responses (`tests/fixtures/hf/`); no test reaches the network.

use std::io::Read;
use std::time::Duration;

use serde_json::{Value, json};

use crate::internal::on_fetch_blocklist;

/// Where the hub talks to Hugging Face. Files may also come from the CDN hosts under `DOMAINS`.
pub const ORIGIN: &str = "https://huggingface.co";
/// The domains the hub may talk to, subdomains included: the API, the files and their CDN.
const DOMAINS: [&str; 2] = ["huggingface.co", "hf.co"];
const TIMEOUT_CONNECT: Duration = Duration::from_secs(30);
const TIMEOUT_RESPONSE: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;
const MAX_JSON_BYTES: u64 = 64 << 20;
pub const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 100;
const SUMMARY_CHARS: usize = 400;
/// The fields the search asks the API to expand (the item shape needs each).
const EXPAND: [&str; 10] = [
    "author",
    "downloads",
    "gated",
    "lastModified",
    "library_name",
    "likes",
    "pipeline_tag",
    "private",
    "tags",
    "sha",
];

/// A failed call: the HTTP status to answer with, a message, and the model page when the owner should look there.
#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub status: u16,
    pub message: String,
    pub url: Option<String>,
}

impl Fail {
    pub fn new(status: u16, message: impl Into<String>) -> Fail {
        Fail {
            status,
            message: message.into(),
            url: None,
        }
    }

    /// Adds the model page to a refusal for `id` (a gated or private model the owner must open).
    pub fn for_model(mut self, id: &str) -> Fail {
        if self.status == 403 && self.url.is_none() {
            self.url = Some(model_page(id));
        }
        self
    }
}

/// The search as the route reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub search: String,
    pub task: String,
    pub library: String,
    pub sort: String,
    pub limit: usize,
    pub cursor: Option<String>,
}

pub fn model_page(id: &str) -> String {
    format!("{ORIGIN}/{id}")
}

/// The API's sort key for the route's `sort`; empty means downloads.
pub fn sort_key(sort: &str) -> Result<&'static str, String> {
    match sort {
        "" | "downloads" => Ok("downloads"),
        "likes" => Ok("likes"),
        "lastModified" => Ok("lastModified"),
        "trending" => Ok("trendingScore"),
        other => Err(format!(
            "sort must be downloads, likes, lastModified or trending, got {other:?}"
        )),
    }
}

/// The page size: 1..=100, default 30.
pub fn page_limit(limit: Option<&str>) -> Result<usize, String> {
    match limit {
        None | Some("") => Ok(DEFAULT_LIMIT),
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| format!("limit must be 1..={MAX_LIMIT}, got {text:?}")),
    }
}

/// The API URL of a search: descending by the sort key, with the fields the item shape needs expanded.
pub fn search_url(q: &Search) -> Result<String, String> {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if !q.search.is_empty() {
        pairs.push(("search", q.search.clone()));
    }
    if !q.task.is_empty() {
        pairs.push(("pipeline_tag", q.task.clone()));
    }
    if !q.library.is_empty() {
        pairs.push(("library", q.library.clone()));
    }
    pairs.push(("sort", sort_key(&q.sort)?.to_string()));
    pairs.push(("direction", "-1".into()));
    pairs.push(("limit", q.limit.to_string()));
    if let Some(cursor) = &q.cursor {
        pairs.push(("cursor", cursor.clone()));
    }
    for field in EXPAND {
        pairs.push(("expand[]", field.to_string()));
    }
    let query: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect();
    Ok(format!("{ORIGIN}/api/models?{}", query.join("&")))
}

/// Percent-encodes everything but the RFC 3986 unreserved characters.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Encodes each segment of a `/`-separated path (a repo id or a file path), keeping the slashes.
pub fn encode_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// Decodes `%XX` escapes (anything else is kept as it is).
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of `name` in a URL's query string, decoded.
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| decode(value))
    })
}

/// The `cursor` of the `rel="next"` link in a Link header, decoded; `None` on the last page.
pub fn next_cursor(link: Option<&str>) -> Option<String> {
    query_param(&next_url(link)?, "cursor")
}

/// The URL of the `rel="next"` link in a Link header, when there is one.
pub fn next_url(link: Option<&str>) -> Option<String> {
    let link = link?;
    let next = link.split(',').find(|part| part.contains("rel=\"next\""))?;
    Some(
        next.split(';')
            .next()?
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string(),
    )
}

/// A query value as the client sent it: `+` and `%XX` are spaces and bytes.
pub fn query_text(raw: &str) -> String {
    decode(&raw.replace('+', " "))
}

/// The commit a revision (branch, tag or sha) names on Hugging Face, and the repo's task (`pipeline_tag`).
pub fn resolve_revision(
    id: &str,
    revision: &str,
    token: Option<&str>,
    block: &[String],
) -> Result<(String, Option<String>), Fail> {
    let url = format!(
        "{ORIGIN}/api/models/{}/revision/{}",
        encode_path(id),
        encode(revision)
    );
    let (value, _) = get_json(&url, token, block, "revision").map_err(|f| f.for_model(id))?;
    let sha = value
        .get("sha")
        .and_then(Value::as_str)
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| Fail::new(502, "Hugging Face gave no commit for the revision"))?;
    Ok((
        sha,
        value
            .get("pipeline_tag")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}

const MAX_TREE_PAGES: usize = 50;

/// Every file of a repo at a commit: the recursive tree, all its pages.
pub fn tree(
    id: &str,
    sha: &str,
    token: Option<&str>,
    block: &[String],
) -> Result<Vec<TreeFile>, Fail> {
    let mut url = format!(
        "{ORIGIN}/api/models/{}/tree/{sha}?recursive=true",
        encode_path(id)
    );
    let mut files = Vec::new();
    for _ in 0..MAX_TREE_PAGES {
        let (value, link) =
            get_json(&url, token, block, "the file tree").map_err(|f| f.for_model(id))?;
        files.extend(parse_tree(&value).map_err(|e| Fail::new(502, e))?);
        match next_url(link.as_deref()) {
            Some(next) => url = next,
            None => return Ok(files),
        }
    }
    Err(Fail::new(502, "the file tree has too many pages"))
}

/// The search reply's items, in the route's compact shape.
pub fn search_items(value: &Value) -> Result<Vec<Value>, String> {
    let list = value.as_array().ok_or("the search reply is not a list")?;
    Ok(list.iter().map(model_summary).collect())
}

/// One page of search results in the compact shape: `{items, next_cursor}`. The route and the breeder's
/// `foundry_hf_search` both answer with it.
pub fn search_page(query: &Search, block: &[String]) -> Result<Value, Fail> {
    let url = search_url(query).map_err(|e| Fail::new(400, e))?;
    let (value, link) = get_json(&url, None, block, "the search")?;
    let items = search_items(&value).map_err(|e| Fail::new(502, e))?;
    Ok(json!({ "items": items, "next_cursor": next_cursor(link.as_deref()) }))
}

/// One model in the compact shape: the search item and the start of the detail.
pub fn model_summary(v: &Value) -> Value {
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let author = v
        .get("author")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| id.split_once('/').map(|(org, _)| org.to_string()));
    json!({
        "id": id,
        "author": author,
        "task": opt_str(v, "pipeline_tag"),
        "library": opt_str(v, "library_name"),
        "tags": v.get("tags").cloned().unwrap_or_else(|| json!([])),
        "downloads": v.get("downloads").and_then(Value::as_u64).unwrap_or(0),
        "likes": v.get("likes").and_then(Value::as_u64).unwrap_or(0),
        "last_modified": opt_str(v, "lastModified"),
        "gated": gated_of(v),
        "private": v.get("private").and_then(Value::as_bool).unwrap_or(false),
        "license": license_of(v),
    })
}

/// The detail's card part: the summary plus the commit sha and the page.
pub fn model_card(v: &Value) -> Value {
    let mut card = model_summary(v);
    let id = card["id"].as_str().unwrap_or("").to_string();
    if let Value::Object(map) = &mut card {
        map.insert("sha".into(), opt_str(v, "sha"));
        map.insert("url".into(), json!(model_page(&id)));
        map.insert("summary".into(), json!(""));
    }
    card
}

fn opt_str(v: &Value, key: &str) -> Value {
    v.get(key)
        .and_then(Value::as_str)
        .map_or(Value::Null, |s| json!(s))
}

/// Gated models say so with `"auto"` or `"manual"`; `false` (or absence) is open.
fn gated_of(v: &Value) -> bool {
    match v.get("gated") {
        Some(Value::Bool(gated)) => *gated,
        Some(Value::String(mode)) => !mode.is_empty(),
        _ => false,
    }
}

fn license_of(v: &Value) -> Value {
    let from_tags = v.get("tags").and_then(Value::as_array).and_then(|tags| {
        tags.iter()
            .filter_map(Value::as_str)
            .find_map(|t| t.strip_prefix("license:"))
    });
    let from_card = v
        .get("cardData")
        .and_then(|c| c.get("license"))
        .and_then(Value::as_str);
    from_tags
        .or(from_card)
        .map_or(Value::Null, |license| json!(license))
}

/// One file of the repo's tree. `sha256` is the LFS oid (SHA-256 of the bytes); git-stored files have none.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeFile {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

/// The files of a recursive tree listing (directories skipped).
pub fn parse_tree(value: &Value) -> Result<Vec<TreeFile>, String> {
    let list = value.as_array().ok_or("the file tree is not a list")?;
    Ok(list
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("file"))
        .filter_map(|e| {
            let path = e.get("path")?.as_str()?.to_string();
            let size = e.get("size").and_then(Value::as_u64).unwrap_or(0);
            let sha256 = e
                .pointer("/lfs/oid")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(TreeFile { path, size, sha256 })
        })
        .collect())
}

pub fn file_json(file: &TreeFile) -> Value {
    json!({ "path": file.path, "size": file.size, "sha256": file.sha256, "lfs": file.sha256.is_some() })
}

/// The ONNX graph's path: `model.onnx` at the top, or `onnx/model.onnx` (the Hugging Face layout).
pub fn graph_path(files: &[TreeFile]) -> Option<&'static str> {
    ["model.onnx", "onnx/model.onnx"]
        .into_iter()
        .find(|p| files.iter().any(|f| f.path == *p))
}

/// What the repo can be imported as. `servable` needs an ONNX graph and either a feature contract
/// (`model_card.json`) or, for a served text task, a `tokenizer.json` (the text runtime, text.rs). Both are
/// checked for real at import.
pub fn importable(files: &[TreeFile], task: Option<&str>) -> Value {
    let has = |name: &str| files.iter().any(|f| f.path == name);
    let text_task = task.is_some_and(super::text::supported);
    let (servable, servable_reason) = if graph_path(files).is_none() {
        (
            false,
            Some("no ONNX graph (model.onnx or onnx/model.onnx) in the repo"),
        )
    } else if has("model_card.json") {
        (true, None)
    } else if has("tokenizer.json") && text_task {
        (true, None)
    } else if has("tokenizer.json") {
        (
            false,
            Some(
                "an ONNX text model is served for text-classification and feature-extraction; this task is not",
            ),
        )
    } else {
        (
            false,
            Some("no feature contract (model_card.json) in the repo"),
        )
    };
    let weights = files
        .iter()
        .any(|f| f.path.ends_with(".safetensors") || f.path == "pytorch_model.bin");
    let (base, base_reason) = if !has("config.json") {
        (false, Some("no config.json in the repo"))
    } else if !weights {
        (
            false,
            Some("no safetensors or pytorch_model.bin weights in the repo"),
        )
    } else {
        (true, None)
    };
    json!({ "servable": servable, "servable_reason": servable_reason, "base": base, "base_reason": base_reason })
}

/// The files an import takes when the request names none: a base model's config, tokenizer and weights,
/// or a servable model's graph, config, feature contract and tokenizer.
pub fn default_files(kind: &str, files: &[TreeFile]) -> Result<Vec<String>, String> {
    let picked: Vec<String> = match kind {
        "base" => files
            .iter()
            .filter(|f| is_base_file(&f.path))
            .map(|f| f.path.clone())
            .collect(),
        "servable" => {
            let graph = graph_path(files)
                .ok_or("no ONNX graph (model.onnx or onnx/model.onnx) in the repo")?;
            let mut picked = vec![graph.to_string()];
            for name in ["config.json", "model_card.json", "tokenizer.json"] {
                if files.iter().any(|f| f.path == name) {
                    picked.push(name.to_string());
                }
            }
            picked
        }
        other => return Err(format!("as must be base or servable, got {other:?}")),
    };
    if picked.is_empty() {
        return Err(format!("the repo has no files for a {kind} import"));
    }
    Ok(picked)
}

/// The default file sets of both kinds, as the detail offers them: what an import of each kind takes
/// (`default_files`), or an empty list where that import would be refused.
pub fn import_files(files: &[TreeFile]) -> Value {
    let set = |kind: &str| default_files(kind, files).unwrap_or_default();
    json!({ "base": set("base"), "servable": set("servable") })
}

fn is_base_file(path: &str) -> bool {
    !path.contains('/')
        && (path == "config.json"
            || path == "generation_config.json"
            || path == "pytorch_model.bin"
            || path == "special_tokens_map.json"
            || path == "vocab.txt"
            || path == "vocab.json"
            || path == "merges.txt"
            || path.starts_with("tokenizer")
            || path.ends_with(".safetensors")
            || path.ends_with(".safetensors.index.json"))
}

/// The README's first paragraph after any front matter, trimmed to 400 characters.
pub fn summary(readme: &str) -> String {
    let mut body = readme;
    if let Some(rest) = readme.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            body = &rest[end + 4..];
        }
    }
    let paragraph: Vec<&str> = body
        .lines()
        .map(str::trim)
        .skip_while(|line| line.is_empty() || line.starts_with('#'))
        .take_while(|line| !line.is_empty())
        .collect();
    let text = paragraph.join(" ");
    if text.chars().count() > SUMMARY_CHARS {
        let cut: String = text.chars().take(SUMMARY_CHARS).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// A file path from a request or the tree: relative, no empty, `.` or `..` parts, no backslash or NUL.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A repo id: `name` or `org/name`, each part letters, digits, `-`, `_` or `.`.
pub fn valid_repo_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('/').collect();
    (1..=2).contains(&parts.len())
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 96
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

/// A revision: a branch, tag or commit, letters, digits and `-_./`, no `..`.
pub fn valid_revision(revision: &str) -> bool {
    !revision.is_empty()
        && revision.len() <= 200
        && !revision.contains("..")
        && !revision.starts_with('/')
        && revision
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}

/// The directory a repo's files at one commit live in: `<org or _>/<name>/<sha>`.
pub fn repo_dir(id: &str, sha: &str) -> String {
    match id.split_once('/') {
        Some((org, name)) => format!("{org}/{name}/{sha}"),
        None => format!("_/{id}/{sha}"),
    }
}

/// The egress rule for the hub's own Hugging Face requests: https on the default port, a Hugging Face
/// domain, and not on the owner's `fetch_block`.
pub fn check_url(url: &str, block: &[String]) -> Result<(), Fail> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|_| Fail::new(502, "a Hugging Face URL did not parse"))?;
    if uri.scheme_str() != Some("https") {
        return Err(Fail::new(502, "a Hugging Face URL must be https"));
    }
    if uri.port_u16().is_some_and(|port| port != 443) {
        return Err(Fail::new(
            502,
            "a Hugging Face URL must use the default https port",
        ));
    }
    let host = uri.host().unwrap_or("").to_ascii_lowercase();
    if !DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return Err(Fail::new(502, format!("{host} is not a Hugging Face host")));
    }
    if on_fetch_blocklist(block, &host) {
        return Err(Fail::new(
            403,
            format!("{host} is on the owner's fetch_block list"),
        ));
    }
    Ok(())
}

/// The URL a redirect's `Location` names, resolved against the URL it came from.
fn resolve(base: &str, location: &str) -> String {
    if location.starts_with("https://") || location.starts_with("http://") {
        return location.to_string();
    }
    if location.starts_with('/') {
        let origin_end = base
            .find("://")
            .and_then(|scheme| base[scheme + 3..].find('/').map(|slash| scheme + 3 + slash))
            .unwrap_or(base.len());
        return format!("{}{location}", &base[..origin_end]);
    }
    location.to_string()
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(TIMEOUT_CONNECT))
        .timeout_recv_response(Some(TIMEOUT_RESPONSE))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .user_agent(concat!("apid-foundry/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(config)
}

/// One GET, redirects followed by hand: every hop passes `check_url`, and the bearer token goes only to
/// `huggingface.co` (never to a CDN host). `offset > 0` asks for the bytes from there on.
fn send(
    url: &str,
    token: Option<&str>,
    offset: u64,
    block: &[String],
) -> Result<ureq::http::Response<ureq::Body>, Fail> {
    let agent = agent();
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        check_url(&current, block)?;
        let uri: ureq::http::Uri = current
            .parse()
            .map_err(|_| Fail::new(502, "a Hugging Face URL did not parse"))?;
        let mut builder = ureq::http::Request::builder()
            .method("GET")
            .uri(current.as_str());
        if uri.host() == Some("huggingface.co") {
            if let Some(token) = token {
                builder = builder.header("Authorization", format!("Bearer {token}"));
            }
        }
        if offset > 0 {
            builder = builder.header("Range", format!("bytes={offset}-"));
        }
        let request = builder
            .body(())
            .map_err(|e| Fail::new(502, format!("bad request: {e}")))?;
        let response = agent
            .run(request)
            .map_err(|e| Fail::new(502, format!("Hugging Face did not answer: {e}")))?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Fail::new(502, "a Hugging Face redirect had no Location"))?;
            current = resolve(&current, location);
            continue;
        }
        return Ok(response);
    }
    Err(Fail::new(502, "too many redirects from Hugging Face"))
}

/// A refused or missing reply, mapped to the route's statuses.
fn refusal(status: u16, what: &str) -> Fail {
    match status {
        401 | 403 => Fail::new(
            403,
            "Hugging Face refused the request: the model does not exist, or it is gated or private. If it is gated, accept its terms on huggingface.co with the owner's account; if it is private, set the token",
        ),
        404 => Fail::new(404, format!("no such {what} on Hugging Face")),
        other => Fail::new(502, format!("Hugging Face answered {other} for {what}")),
    }
}

/// GET of a JSON document: the parsed body and the Link header.
pub fn get_json(
    url: &str,
    token: Option<&str>,
    block: &[String],
    what: &str,
) -> Result<(Value, Option<String>), Fail> {
    let response = send(url, token, 0, block)?;
    let status = response.status().as_u16();
    let link = response
        .headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !(200..300).contains(&status) {
        return Err(refusal(status, what));
    }
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_JSON_BYTES)
        .read_to_string(&mut text)
        .map_err(|e| Fail::new(502, format!("reading Hugging Face's reply: {e}")))?;
    let value = serde_json::from_str(&text)
        .map_err(|e| Fail::new(502, format!("Hugging Face sent no JSON: {e}")))?;
    Ok((value, link))
}

/// GET of a text file (a README): `None` when the repo has none.
pub fn get_text_optional(
    url: &str,
    token: Option<&str>,
    block: &[String],
) -> Result<Option<String>, Fail> {
    let response = send(url, token, 0, block)?;
    let status = response.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        return Err(refusal(status, "the README"));
    }
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_JSON_BYTES)
        .read_to_string(&mut text)
        .map_err(|e| Fail::new(502, format!("reading the README: {e}")))?;
    Ok(Some(text))
}

/// The bytes of one file at a commit, from `offset` on. `status` is 200 (whole file) or 206 (a range).
pub struct Stream {
    pub status: u16,
    pub reader: Box<dyn Read>,
}

/// Opens one file of `id` at `sha`. A 416 means the offset is past the end; the caller starts over.
pub fn open_file(
    id: &str,
    sha: &str,
    path: &str,
    offset: u64,
    token: Option<&str>,
    block: &[String],
) -> Result<Stream, Fail> {
    let url = format!(
        "{ORIGIN}/{}/resolve/{sha}/{}",
        encode_path(id),
        encode_path(path)
    );
    let response = send(&url, token, offset, block).map_err(|f| f.for_model(id))?;
    let status = response.status().as_u16();
    match status {
        200 | 206 => Ok(Stream {
            status,
            reader: Box::new(response.into_body().into_reader()),
        }),
        416 => Err(Fail::new(416, format!("{path} has no bytes past {offset}"))),
        other => Err(refusal(other, path).for_model(id)),
    }
}

/// The detail of a model at a revision: the card, the file list, the README summary and the importable hints.
pub fn detail(
    id: &str,
    revision: &str,
    token: Option<&str>,
    block: &[String],
) -> Result<Value, Fail> {
    let url = format!(
        "{ORIGIN}/api/models/{}/revision/{}",
        encode_path(id),
        encode(revision)
    );
    let (value, _) = get_json(&url, token, block, "revision").map_err(|f| f.for_model(id))?;
    let mut card = model_card(&value);
    let sha = card["sha"].as_str().unwrap_or("").to_string();
    if sha.is_empty() {
        return Err(Fail::new(
            502,
            "Hugging Face gave no commit for the revision",
        ));
    }
    let files = tree(id, &sha, token, block)?;
    let summary = readme_summary(id, &sha, token, block)?;
    let task = card["task"].as_str().map(str::to_string);
    if let Value::Object(map) = &mut card {
        map.insert("summary".into(), json!(summary));
        map.insert(
            "files".into(),
            Value::Array(files.iter().map(file_json).collect()),
        );
        map.insert("importable".into(), importable(&files, task.as_deref()));
        map.insert("import_files".into(), import_files(&files));
    }
    Ok(card)
}

/// The detail's README summary, fetched at the commit (`""` when the repo has none).
pub fn readme_summary(
    id: &str,
    sha: &str,
    token: Option<&str>,
    block: &[String],
) -> Result<String, Fail> {
    let url = format!("{ORIGIN}/{}/raw/{sha}/README.md", encode_path(id));
    Ok(get_text_optional(&url, token, block)?
        .map(|text| summary(&text))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../../tests/fixtures/hf/search.json");
    const MODEL: &str = include_str!("../../tests/fixtures/hf/model.json");
    const TREE: &str = include_str!("../../tests/fixtures/hf/tree.json");

    fn json(text: &str) -> Value {
        serde_json::from_str(text).expect("fixture parses")
    }

    fn files() -> Vec<TreeFile> {
        parse_tree(&json(TREE)).expect("tree parses")
    }

    fn search(query: &str) -> Search {
        Search {
            search: query.into(),
            limit: DEFAULT_LIMIT,
            sort: "downloads".into(),
            ..Search::default()
        }
    }

    #[test]
    fn search_urls_encode_the_query_and_map_the_sort() {
        let url = search_url(&Search {
            search: "sst2 onnx".into(),
            task: "text-classification".into(),
            ..search("")
        })
        .unwrap();
        assert!(
            url.starts_with("https://huggingface.co/api/models?"),
            "{url}"
        );
        assert!(url.contains("search=sst2%20onnx"), "{url}");
        assert!(url.contains("pipeline_tag=text-classification"), "{url}");
        assert!(
            url.contains("sort=downloads&direction=-1&limit=30"),
            "{url}"
        );
        let trending = search_url(&Search {
            sort: "trending".into(),
            ..search("")
        })
        .unwrap();
        assert!(trending.contains("sort=trendingScore"), "{trending}");
        assert!(
            search_url(&Search {
                sort: "stars".into(),
                ..search("")
            })
            .is_err()
        );
    }

    #[test]
    fn the_limit_is_1_to_100_and_defaults_to_30() {
        assert_eq!(page_limit(None), Ok(30));
        assert_eq!(page_limit(Some("100")), Ok(100));
        assert!(page_limit(Some("0")).is_err());
        assert!(page_limit(Some("101")).is_err());
        assert!(page_limit(Some("many")).is_err());
    }

    #[test]
    fn the_next_cursor_is_read_from_the_link_header_and_decoded() {
        let link = "<https://huggingface.co/api/models?search=sst2+onnx&limit=2&cursor=ab%3Dc%2Bd>; rel=\"next\"";
        assert_eq!(next_cursor(Some(link)).as_deref(), Some("ab=c+d"));
        assert_eq!(next_cursor(None), None);
        assert_eq!(
            next_cursor(Some(
                "<https://huggingface.co/api/models?limit=2>; rel=\"prev\""
            )),
            None
        );
        let url = search_url(&Search {
            cursor: Some("ab=c+d".into()),
            ..search("")
        })
        .unwrap();
        assert!(url.contains("cursor=ab%3Dc%2Bd"), "{url}");
    }

    #[test]
    fn search_items_map_to_the_compact_shape() {
        let items = search_items(&json(SEARCH)).unwrap();
        assert_eq!(items.len(), 2);
        let first = &items[0];
        assert_eq!(
            first["id"],
            "fxmarty/distilbert-base-uncased-sst2-onnx-int8-for-tensorrt"
        );
        assert_eq!(first["author"], "fxmarty");
        assert_eq!(first["task"], "text-classification");
        assert_eq!(first["library"], "transformers");
        assert_eq!(first["license"], "mit");
        assert_eq!(first["downloads"], 8);
        assert_eq!(first["gated"], false);
        assert_eq!(first["private"], false);
        assert!(
            first["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t == "onnx")
        );
        assert!(search_items(&json("{}")).is_err());
    }

    #[test]
    fn a_gated_model_is_flagged_by_its_mode_or_flag() {
        assert!(gated_of(&json(r#"{"gated":"manual"}"#)));
        assert!(gated_of(&json(r#"{"gated":true}"#)));
        assert!(!gated_of(&json(r#"{"gated":false}"#)));
        assert!(!gated_of(&json("{}")));
    }

    #[test]
    fn the_model_card_carries_the_commit_and_the_page() {
        let card = model_card(&json(MODEL));
        assert_eq!(
            card["id"],
            "Xenova/distilbert-base-uncased-finetuned-sst-2-english"
        );
        assert_eq!(card["sha"], "0b6928efcb76139cae2c6881d49cda67fe119f42");
        assert_eq!(
            card["url"],
            "https://huggingface.co/Xenova/distilbert-base-uncased-finetuned-sst-2-english"
        );
        assert_eq!(card["task"], "text-classification");
        assert_eq!(
            card["license"],
            Value::Null,
            "the detail's tags carry no license here"
        );
        assert_eq!(card["downloads"], 26244);
    }

    #[test]
    fn the_tree_lists_files_with_their_lfs_sha256_and_skips_directories() {
        let files = files();
        assert!(files.iter().all(|f| !f.path.is_empty()));
        assert!(
            !files.iter().any(|f| f.path == "onnx"),
            "directories are skipped"
        );
        let graph = files.iter().find(|f| f.path == "onnx/model.onnx").unwrap();
        assert_eq!(graph.size, 267_955_711);
        assert_eq!(
            graph.sha256.as_deref(),
            Some("252cf7048af94a1599019fef35961b2bd3d6db13df0b0a4b032b92baeae31939")
        );
        let config = files.iter().find(|f| f.path == "config.json").unwrap();
        assert_eq!(config.sha256, None);
        assert_eq!(file_json(graph)["lfs"], true);
    }

    #[test]
    fn importable_hints_say_what_the_repo_can_be() {
        let hints = importable(&files(), Some("text-classification"));
        // An ONNX text model with a tokenizer for a served task: servable (the text runtime).
        assert_eq!(hints["servable"], true);
        assert_eq!(hints["servable_reason"], Value::Null);
        // The same repo for a task the runtime does not serve: not servable, and the reason says so.
        let other = importable(&files(), Some("translation"));
        assert_eq!(other["servable"], false);
        assert!(other["servable_reason"].as_str().unwrap().contains("task"));
        assert_eq!(importable(&files(), None)["servable"], false);
        // No safetensors or PyTorch weights: not a base model.
        assert_eq!(hints["base"], false);
        assert!(hints["base_reason"].as_str().unwrap().contains("weights"));
    }

    #[test]
    fn importable_is_true_for_a_contract_with_a_graph_and_for_weights_with_a_config() {
        let file = |path: &str| TreeFile {
            path: path.into(),
            size: 1,
            sha256: None,
        };
        let servable = importable(&[file("onnx/model.onnx"), file("model_card.json")], None);
        assert_eq!(servable["servable"], true);
        assert_eq!(servable["servable_reason"], Value::Null);
        let base = importable(
            &[file("config.json"), file("model-00001.safetensors")],
            None,
        );
        assert_eq!(base["base"], true);
        assert_eq!(base["base_reason"], Value::Null);
        assert_eq!(importable(&[file("model.onnx")], None)["servable"], false);
    }

    #[test]
    fn default_files_and_the_import_offer_pick_the_graph_and_the_base_files() {
        let files = files();
        assert_eq!(
            default_files("servable", &files).unwrap(),
            vec![
                "onnx/model.onnx".to_string(),
                "config.json".to_string(),
                "tokenizer.json".to_string()
            ]
        );
        let base = default_files("base", &files).unwrap();
        assert!(
            base.contains(&"config.json".to_string())
                && base.contains(&"tokenizer.json".to_string())
        );
        assert!(!base.iter().any(|p| p.starts_with("onnx/")), "{base:?}");
        assert!(default_files("tensors", &files).is_err());
        assert!(default_files("servable", &[]).is_err());
        // The import offer carries both sets, and is empty where an import is refused.
        let sets = import_files(&files);
        assert_eq!(
            sets["servable"],
            json!(default_files("servable", &files).unwrap())
        );
        assert_eq!(sets["base"], json!(base));
        assert_eq!(import_files(&[]), json!({ "base": [], "servable": [] }));
    }

    #[test]
    fn paths_repo_ids_and_revisions_are_checked() {
        assert!(safe_path("onnx/model.onnx"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "a\\b",
            "a/./b",
            "a\0b",
        ] {
            assert!(!safe_path(bad), "{bad:?} must be refused");
        }
        assert!(valid_repo_id("gpt2"));
        assert!(valid_repo_id(
            "Xenova/distilbert-base-uncased-finetuned-sst-2-english"
        ));
        for bad in ["", "a/b/c", "../x", "a/", "a b", "a?x=1"] {
            assert!(!valid_repo_id(bad), "{bad:?}");
        }
        assert!(
            valid_revision("main") && valid_revision("0b6928efcb76139cae2c6881d49cda67fe119f42")
        );
        assert!(!valid_revision("main..x") && !valid_revision("a b"));
        assert_eq!(repo_dir("Xenova/x", "abc"), "Xenova/x/abc");
        assert_eq!(repo_dir("gpt2", "abc"), "_/gpt2/abc");
    }

    #[test]
    fn egress_allows_hugging_face_hosts_only_over_https_and_respects_the_block_list() {
        let none: Vec<String> = Vec::new();
        for ok in [
            "https://huggingface.co/api/models",
            "https://cdn-lfs-us-1.hf.co/repos/x",
            "https://cas-bridge.xethub.hf.co/xet/y",
        ] {
            assert!(check_url(ok, &none).is_ok(), "{ok}");
        }
        for bad in [
            "http://huggingface.co/x",
            "https://evil.example/x",
            "https://huggingface.co.evil.example/x",
            "https://huggingface.co:8443/x",
            "file:///etc/passwd",
        ] {
            assert!(check_url(bad, &none).is_err(), "{bad}");
        }
        let blocked = vec!["hf.co".to_string()];
        assert_eq!(
            check_url("https://cdn-lfs.hf.co/x", &blocked)
                .unwrap_err()
                .status,
            403
        );
        assert!(check_url("https://huggingface.co/x", &blocked).is_ok());
    }

    #[test]
    fn redirects_resolve_against_their_origin() {
        assert_eq!(
            resolve("https://huggingface.co/a/b", "/x/y"),
            "https://huggingface.co/x/y"
        );
        assert_eq!(
            resolve("https://huggingface.co/a", "https://cdn-lfs.hf.co/z"),
            "https://cdn-lfs.hf.co/z"
        );
    }
}
