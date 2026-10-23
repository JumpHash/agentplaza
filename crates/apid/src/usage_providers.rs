//! Usage providers (`/v1/usage-providers`): servers that pool model accounts and report their use, shown
//! beside the providers. Today one kind, a CLIProxyAPI hub: the owner gives its address and its management
//! key, the key goes into the vault (`usage:<id>`), and the hub reads `GET <url>/v0/management/usage` and
//! `GET <url>/v0/management/auth-files` with it. The key never leaves the hub, and the answers are reduced
//! to totals, models and accounts before a console sees them (CLIProxyAPI keys its per-API figures by the
//! client key in the vault): that is how subscription accounts reach agents without a key in any VM.
//!
//! The providers are saved in plaza.db (table `usage_provider`); their keys stay in the vault.

use std::sync::Mutex;
use std::time::Duration;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::providers::{Failure, failure, lock};
use crate::state::App;
use crate::store::saver::save_now;
use crate::vault::{Consumer, OnBehalf, name_for};

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const OWNER: &str = "owner";

/// One usage provider as stored: no key, only where it is. Its row is keyed by `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "usage_provider")]
pub struct UsageProvider {
    /// `[a-z0-9_-]`, at most 40 characters.
    #[table(key)]
    pub id: String,
    pub kind: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    #[serde(default)]
    id: Option<String>,
    #[serde(default = "cliproxyapi")]
    kind: String,
    url: String,
    #[serde(default)]
    label: Option<String>,
    key: String,
}

fn cliproxyapi() -> String {
    "cliproxyapi".into()
}

/// The usage providers and their last readings. Held in memory; the providers are saved in plaza.db.
#[derive(Default)]
pub struct Store {
    list: Mutex<Vec<UsageProvider>>,
    /// The last reading of each provider, by id.
    last: Mutex<std::collections::BTreeMap<String, Value>>,
}

impl Store {
    /// The providers' rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let items = lock(&self.list);
        RowSet::list(def(), items.iter())
    }

    /// Replaces the providers with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<UsageProvider> = apz_store::table::load_list(conn, def())?;
        *lock(&self.list) = stored;
        Ok(())
    }

    fn get(&self, id: &str) -> Option<UsageProvider> {
        lock(&self.list).iter().find(|p| p.id == id).cloned()
    }
}

/// The usage providers' table: `usage_provider`, one row per provider.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(UsageProvider)
}

/// Replaces the providers and writes them now, so the owner's answer comes after the write. When the write
/// fails the providers stay as they were.
fn save(app: &App, list: Vec<UsageProvider>) -> Result<(), Failure> {
    let before = std::mem::replace(&mut *lock(&app.usage_providers.list), list);
    if save_now(app, "usage_providers") {
        return Ok(());
    }
    *lock(&app.usage_providers.list) = before;
    Err((
        500,
        "write usage providers: the store did not save them".into(),
    ))
}

/// The vault name and binding of a usage provider's management key.
fn key_name(id: &str) -> String {
    name_for(&format!("usage-{id}-key"))
}

fn binding(id: &str) -> String {
    format!("usage:{id}")
}

/// A usage provider's view: where it is, whether its key is held, and its last reading.
fn view(app: &App, p: &UsageProvider) -> Value {
    let label = p.label.clone().unwrap_or_else(|| host_of(&p.url));
    json!({
        "id": p.id,
        "kind": p.kind,
        "url": p.url,
        "label": label,
        "key": app.vault.presence(&key_name(&p.id), &binding(&p.id)),
        "last": lock(&app.usage_providers.last).get(&p.id),
    })
}

/// The host of `url`, or the url itself.
fn host_of(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?'])
        .next()
        .unwrap_or(url)
        .to_string()
}

/// An id from a hub's host name: `hub.example.ts.net:8318` → `hub-example-ts-net`.
fn id_from(url: &str) -> String {
    let host = host_of(url);
    let host = host.split(':').next().unwrap_or("");
    let id: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(40)
        .collect();
    if id.is_empty() {
        "cliproxyapi".into()
    } else {
        id
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// An http or https address with a host, no credentials in it, no query or fragment.
fn check_url(url: &str) -> Result<String, Failure> {
    let url = url.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or((400, "url must start with http:// or https://".to_string()))?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty() || host.contains('@') || url.contains(['?', '#', ' ']) || url.len() > 300 {
        return Err((
            400,
            "url must be a plain address such as https://hub.example.ts.net:8318".into(),
        ));
    }
    Ok(url.to_string())
}

/// GET `<base><path>` with the management key; the JSON answer, or why it failed (never the key).
fn management_get(base: &str, path: &str, key: &SecretString) -> Result<Value, String> {
    let url = format!("{base}{path}");
    let request = ureq::http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("user-agent", "agentplaza")
        .header("authorization", format!("Bearer {}", key.expose_secret()))
        .body(Vec::new())
        .map_err(|e| format!("bad request: {e}"))?;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .http_status_as_error(false)
        .build();
    let mut response = ureq::Agent::new_with_config(config)
        .run(request)
        .map_err(|e| format!("{path} did not answer: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err("the management key was refused".into()),
        404 => return Err("remote management is off on that hub (no secret-key set), or it is not a CLIProxyAPI hub".into()),
        status => return Err(format!("{path} answered {status}")),
    }
    let text = response
        .body_mut()
        .with_config()
        .limit(8 << 20)
        .read_to_string()
        .map_err(|e| format!("reading {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path} is not JSON: {e}"))
}

/// CLIProxyAPI's `/usage` and `/auth-files` answers, reduced to totals, models and accounts. Client keys
/// (the keys of its per-API map) are dropped.
fn summarise(usage: &Value, auth_files: Option<&Value>) -> Value {
    let u = usage.get("usage").unwrap_or(usage);
    let num = |v: &Value| v.as_u64().unwrap_or(0);
    let mut models: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    if let Some(apis) = u["apis"].as_object() {
        for api in apis.values() {
            if let Some(ms) = api["models"].as_object() {
                for (model, m) in ms {
                    let e = models.entry(model.clone()).or_default();
                    e.0 += num(&m["total_requests"]);
                    e.1 += num(&m["total_tokens"]);
                }
            }
        }
    }
    let mut models: Vec<Value> = models
        .into_iter()
        .map(|(model, (requests, tokens))| json!({ "model": model, "requests": requests, "tokens": tokens }))
        .collect();
    models.sort_by_key(|m| std::cmp::Reverse(m["requests"].as_u64().unwrap_or(0)));
    let accounts: Vec<Value> = auth_files
        .and_then(|a| a.get("files").or(Some(a)))
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .map(|f| {
                    json!({
                        "name": f["label"].as_str().or(f["email"].as_str()).or(f["name"].as_str()).unwrap_or(""),
                        "provider": f["provider"].as_str().or(f["type"].as_str()).unwrap_or(""),
                        "status": f["status"].as_str().unwrap_or(if f["disabled"].as_bool() == Some(true) { "disabled" } else { "active" }),
                        "message": f["status_message"].as_str().unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "totals": {
            "requests": num(&u["total_requests"]),
            "success": num(&u["success_count"]),
            "failure": num(&u["failure_count"]),
            "tokens": num(&u["total_tokens"]),
        },
        "models": models,
        "accounts": accounts,
    })
}

/// Reads provider `p` now, with its key from the vault, and keeps the reading.
fn read(app: &App, p: &UsageProvider) -> Value {
    let checked_ms = apz_plaza::now_ms();
    let consumer = Consumer {
        binding: binding(&p.id),
        on_behalf: OnBehalf::Owner { node: None },
    };
    let reading = match app.vault.resolve(&key_name(&p.id), &consumer, checked_ms) {
        Err(e) => {
            json!({ "status": "crit", "message": format!("The management key cannot be read: {}", e.message()) })
        }
        Ok(key) => match management_get(&p.url, "/v0/management/usage", &key) {
            Err(e) => json!({ "status": "crit", "message": e }),
            Ok(usage) => {
                let files = management_get(&p.url, "/v0/management/auth-files", &key).ok();
                let mut s = summarise(&usage, files.as_ref());
                s["status"] = json!("ok");
                let n = s["accounts"].as_array().map_or(0, Vec::len);
                s["message"] = json!(format!(
                    "{n} {} pooled",
                    if n == 1 { "account" } else { "accounts" }
                ));
                s
            }
        },
    };
    let mut reading = reading;
    reading["checked_ms"] = json!(checked_ms);
    lock(&app.usage_providers.last).insert(p.id.clone(), reading.clone());
    reading
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/usage-providers", list_route)
        .doc(RouteDoc::new(
            "List the usage providers (CLIProxyAPI hubs) and their last readings",
        ))
        .post("/v1/usage-providers", create_route)
        .doc(
            RouteDoc::new("Add a usage provider: {url, key, label?}; the key goes into the vault")
                .status(201)
                .outbound()
                .errors(&[400, 409]),
        )
        .get("/v1/usage-providers/{id}/usage", usage_route)
        .doc(
            RouteDoc::new("Read a usage provider now: totals, models and pooled accounts")
                .outbound()
                .errors(&[404]),
        )
        .delete("/v1/usage-providers/{id}", delete_route)
        .doc(RouteDoc::new("Remove a usage provider and its key").errors(&[404]))
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let list = lock(&app.usage_providers.list).clone();
    Response::json(200, &list.iter().map(|p| view(app, p)).collect::<Vec<_>>())
}

fn create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: Create = match req.json() {
        Ok(b) => b,
        Err(e) => return failure((400, format!("bad body: {e}"))),
    };
    if body.kind != "cliproxyapi" {
        return failure((400, "kind must be cliproxyapi".into()));
    }
    let url = match check_url(&body.url) {
        Ok(u) => u,
        Err(f) => return failure(f),
    };
    let id = body.id.unwrap_or_else(|| id_from(&url));
    if !valid_id(&id) {
        return failure((400, "id must match [a-z0-9_-]{1,40}".into()));
    }
    if body.key.trim().is_empty() || body.key.len() > 4096 {
        return failure((400, "the management key is required".into()));
    }
    if let Some(l) = &body.label
        && (l.trim().is_empty() || l.chars().count() > 60)
    {
        return failure((400, "label must be 1 to 60 characters".into()));
    }
    let mut list = lock(&app.usage_providers.list).clone();
    if list.iter().any(|p| p.id == id) {
        return failure((409, format!("usage provider `{id}` exists")));
    }
    let secret = SecretString::from(body.key.trim().to_string());
    if let Err(e) = app.vault.put_plaza(
        &key_name(&id),
        &binding(&id),
        "usage provider management key",
        &secret,
        OWNER,
        apz_plaza::now_ms(),
    ) {
        return failure((409, format!("the vault refused the key: {}", e.message())));
    }
    let p = UsageProvider {
        id,
        kind: body.kind,
        url,
        label: body.label,
    };
    list.push(p.clone());
    if let Err(f) = save(app, list) {
        return failure(f);
    }
    read(app, &p);
    Response::json(201, &view(app, &p))
}

fn usage_route(app: &App, _: &Request, params: &Params<'_>) -> Response {
    match app.usage_providers.get(params.get("id")) {
        Some(p) => Response::json(200, &read(app, &p)),
        None => failure((404, format!("no usage provider `{}`", params.get("id")))),
    }
}

fn delete_route(app: &App, _: &Request, params: &Params<'_>) -> Response {
    let id = params.get("id");
    let mut list = lock(&app.usage_providers.list).clone();
    let before = list.len();
    list.retain(|p| p.id != id);
    if list.len() == before {
        return failure((404, format!("no usage provider `{id}`")));
    }
    if let Err(f) = save(app, list) {
        return failure(f);
    }
    let _ = app
        .vault
        .remove_binding(&binding(id), OWNER, apz_plaza::now_ms());
    lock(&app.usage_providers.last).remove(id);
    Response::json(200, &json!({ "deleted": id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_urls() {
        assert_eq!(
            id_from("https://hub.example.ts.net:8318"),
            "hub-example-ts-net"
        );
        assert_eq!(
            check_url("https://hub.example.ts.net:8318/").unwrap(),
            "https://hub.example.ts.net:8318"
        );
        assert!(check_url("ftp://x").is_err());
        assert!(check_url("https://user:pw@x").is_err());
        assert!(check_url("https://x/?k=1").is_err());
    }

    #[test]
    fn the_providers_come_back_whole_from_their_table() {
        let store = Store::default();
        *lock(&store.list) = vec![
            UsageProvider {
                id: "hub-a".into(),
                kind: "cliproxyapi".into(),
                url: "https://hub.example.ts.net:8318".into(),
                label: Some("Hub A".into()),
            },
            UsageProvider {
                id: "hub-b".into(),
                kind: "cliproxyapi".into(),
                url: "https://b.example.ts.net".into(),
                label: None,
            },
        ];
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();

        let back = Store::default();
        back.load(&conn).unwrap();
        assert_eq!(*lock(&back.list), *lock(&store.list));
    }

    #[test]
    fn readings_drop_the_client_keys() {
        let usage = json!({"usage": {"total_requests": 12, "success_count": 11, "failure_count": 1, "total_tokens": 900,
            "apis": {"sk-client-secret": {"models": {"gpt-6": {"total_requests": 7, "total_tokens": 500}, "claude": {"total_requests": 5, "total_tokens": 400}}}}}});
        let files = json!({"files": [{"email": "me@example.com", "provider": "codex", "status": "active"}, {"name": "c.json", "type": "claude", "disabled": true}]});
        let s = summarise(&usage, Some(&files));
        assert!(!s.to_string().contains("sk-client-secret"));
        assert_eq!(s["totals"]["requests"], 12);
        assert_eq!(s["models"][0]["model"], "gpt-6");
        assert_eq!(s["accounts"][1]["status"], "disabled");
        assert_eq!(s["accounts"][0]["provider"], "codex");
    }
}
