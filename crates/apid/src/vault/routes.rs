//! The vault's HTTP routes (docs/design/vault.md). Every `/v1` path is behind the owner's gate
//! (`hub_auth::gate`); nothing is mounted under `/internal`, which is the agent gateway's. No handler
//! echoes a body, and no response, event or error carries a value.

use std::collections::BTreeMap;

use apz_http::{Request, Response, RouteDoc, Router};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{Value, json};

use super::model::Scope;
use super::{LOCAL_NODE, NewSecret, OnBehalf, Store, Unlock, VaultError};
use crate::events::Bus;
use crate::nodes::{self, Node};
use crate::state::App;

/// What the handlers need: the vault, the event bus, and the node table. Tests build one without an App.
pub struct Ctx<'a> {
    pub store: &'a Store,
    pub bus: &'a Bus,
    pub nodes: &'a nodes::Store,
}

fn ctx(app: &App) -> Ctx<'_> {
    Ctx {
        store: &app.vault,
        bus: &app.bus,
        nodes: &app.nodes,
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/vault", |app, _, _| status(&ctx(app)))
        .doc(RouteDoc::new("Show the vault's state"))
        .post("/v1/vault/unseal", |app, req, _| unseal(&ctx(app), req))
        .doc(RouteDoc::new("Unseal the vault").errors(&[400, 409, 429, 503]))
        .post("/v1/vault/seal", |app, _, _| seal(&ctx(app)))
        .doc(RouteDoc::new("Seal the vault").errors(&[409, 423, 503]))
        .post("/v1/vault/slots", |app, req, _| add_slot(&ctx(app), req))
        .doc(RouteDoc::new("Add an unlock slot").errors(&[400, 409, 423, 503]))
        .delete("/v1/vault/slots/{id}", |app, _, p| {
            remove_slot(&ctx(app), p.get("id"))
        })
        .doc(RouteDoc::new("Remove an unlock slot").errors(&[404, 409, 423, 503]))
        .post("/v1/vault/recovery", |app, _, _| recovery(&ctx(app)))
        .doc(RouteDoc::new("Make a new recovery code, shown once").errors(&[409, 423, 503]))
        .post("/v1/vault/rekey", |app, req, _| rekey(&ctx(app), req))
        .doc(RouteDoc::new("Rekey the vault").errors(&[400, 409, 423, 503]))
        .get("/v1/vault/secrets", |app, _, _| list(&ctx(app)))
        .doc(RouteDoc::new("List the vault's entries").errors(&[409, 423, 503]))
        .post("/v1/vault/secrets", |app, req, _| create(&ctx(app), req))
        .doc(RouteDoc::new("Create a secret").errors(&[400, 409, 423, 503]))
        .route("PUT", "/v1/vault/secrets/{id}/value", |app, req, p| {
            rotate(&ctx(app), p.get("id"), req)
        })
        .doc(RouteDoc::new("Rotate a secret's value").errors(&[400, 404, 409, 423, 503]))
        .route("PATCH", "/v1/vault/secrets/{id}", |app, req, p| {
            patch(&ctx(app), p.get("id"), req)
        })
        .doc(
            RouteDoc::new("Change a secret's label, scope or bindings")
                .errors(&[400, 404, 409, 423, 503]),
        )
        .post("/v1/vault/secrets/{id}/rollback", |app, req, p| {
            rollback(&ctx(app), p.get("id"), req)
        })
        .doc(
            RouteDoc::new("Roll a secret back to an earlier version")
                .errors(&[400, 404, 409, 423, 503]),
        )
        .delete("/v1/vault/secrets/{id}", |app, _, p| {
            delete(&ctx(app), p.get("id"))
        })
        .doc(
            RouteDoc::new("Delete a secret")
                .status(204)
                .errors(&[404, 409, 423, 503]),
        )
        .post("/v1/vault/secrets/{id}/test", |app, req, p| {
            test(&ctx(app), p.get("id"), req, |binding, node| {
                scrubbed(app, binding, node)
            })
        })
        .doc(
            RouteDoc::new("Check a secret with its bound consumer")
                .errors(&[400, 404, 409, 423, 503]),
        )
        .post("/v1/vault/import", |app, req, _| import(&ctx(app), req))
        .doc(RouteDoc::new("Import secrets from a dotenv file").errors(&[400, 409, 423, 503]))
        .get("/v1/vault/consumers", |app, _, _| consumers(&ctx(app)))
        .doc(RouteDoc::new("List each consumer and its resolved entry").errors(&[409, 423, 503]))
        .get("/v1/vault/audit", |app, req, _| audit(&ctx(app), req))
        .doc(
            RouteDoc::new("Read the audit log, verifying it on request")
                .errors(&[400, 409, 423, 503]),
        )
}

fn now() -> u64 {
    apz_plaza::now_ms()
}

/// The status of a route's error (`VaultError::status`), with the text that may be shown: never a value.
fn fail(err: VaultError) -> Response {
    Response::error(err.status(), &err.message())
}

fn parse<T: for<'de> Deserialize<'de>>(req: &Request) -> Result<T, Response> {
    req.json::<T>()
        .map_err(|_| Response::error(400, "the body is not the JSON this route takes"))
}

/// The body of a route whose body is optional: an empty body is the default.
fn parse_or_default<T: for<'de> Deserialize<'de> + Default>(req: &Request) -> Result<T, Response> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    parse(req)
}

fn nodes_of(ctx: &Ctx) -> Vec<Node> {
    ctx.nodes.list()
}

/// Every mutation says which entries changed (ids only) on the event stream.
fn changed(ctx: &Ctx, ids: &[String]) {
    ctx.bus.publish("vault.changed", &json!({ "ids": ids }));
}

fn status_response(ctx: &Ctx) -> Response {
    Response::json(200, &ctx.store.status(&nodes_of(ctx)))
}

fn view_response(ctx: &Ctx, id: &str) -> Response {
    match ctx.store.view(id) {
        Ok(view) => Response::json(200, &view),
        Err(err) => fail(err),
    }
}

/// `GET /v1/vault`: the state, the slots, the entries, the nodes and the audit's last known state.
pub(super) fn status(ctx: &Ctx) -> Response {
    status_response(ctx)
}

#[derive(Deserialize)]
pub(super) struct UnsealBody {
    passphrase: Option<String>,
    recovery_code: Option<String>,
}

/// `POST /v1/vault/unseal {passphrase | recovery_code}`; an empty body tries the key file.
pub(super) fn unseal(ctx: &Ctx, req: &Request) -> Response {
    let body: UnsealBody = match parse_or_default_unseal(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let unlock = match (body.passphrase, body.recovery_code) {
        (Some(passphrase), None) => Unlock::Passphrase(SecretString::from(passphrase)),
        (None, Some(code)) => Unlock::Recovery(SecretString::from(code)),
        (None, None) => Unlock::Keyfile,
        _ => return Response::error(400, "give a passphrase or a recovery code, not both"),
    };
    match ctx.store.unseal(unlock, "owner", now()) {
        Ok(()) => {
            changed(ctx, &[]);
            status_response(ctx)
        }
        Err(err) => fail(err),
    }
}

fn parse_or_default_unseal(req: &Request) -> Result<UnsealBody, Response> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(UnsealBody {
            passphrase: None,
            recovery_code: None,
        });
    }
    parse(req)
}

/// `POST /v1/vault/seal`.
pub(super) fn seal(ctx: &Ctx) -> Response {
    match ctx.store.seal("owner", now()) {
        Ok(()) => {
            changed(ctx, &[]);
            status_response(ctx)
        }
        Err(err) => fail(err),
    }
}

#[derive(Deserialize)]
pub(super) struct SlotBody {
    kind: String,
    passphrase: Option<String>,
}

/// `POST /v1/vault/slots {kind:"passphrase", passphrase} | {kind:"keyfile"}`.
pub(super) fn add_slot(ctx: &Ctx, req: &Request) -> Response {
    let body: SlotBody = match parse(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let result = match body.kind.as_str() {
        "passphrase" => match body.passphrase {
            Some(passphrase) => {
                ctx.store
                    .add_passphrase(&SecretString::from(passphrase), "owner", now())
            }
            None => Err(VaultError::Invalid(
                "a passphrase slot needs a passphrase".into(),
            )),
        },
        "keyfile" => ctx.store.add_keyfile("owner", now()),
        _ => Err(VaultError::Invalid(
            "a slot is kind passphrase or keyfile".into(),
        )),
    };
    match result {
        Ok(_) => {
            changed(ctx, &[]);
            status_response(ctx)
        }
        Err(err) => fail(err),
    }
}

/// `DELETE /v1/vault/slots/{id}`.
pub(super) fn remove_slot(ctx: &Ctx, id: &str) -> Response {
    match ctx.store.remove_slot(id, "owner", now()) {
        Ok(()) => {
            changed(ctx, &[]);
            status_response(ctx)
        }
        Err(err) => fail(err),
    }
}

/// `POST /v1/vault/recovery`: the new code, shown once.
pub(super) fn recovery(ctx: &Ctx) -> Response {
    match ctx.store.make_recovery("owner", now()) {
        Ok(code) => {
            changed(ctx, &[]);
            Response::json(200, &json!({ "recovery_code": code.as_str() }))
        }
        Err(err) => fail(err),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct RekeyBody {
    /// The passphrase or recovery code of each other passphrase/recovery slot, by slot id.
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// `POST /v1/vault/rekey`.
pub(super) fn rekey(ctx: &Ctx, req: &Request) -> Response {
    let body: RekeyBody = match parse_or_default(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let supplied: Vec<(String, SecretString)> = body
        .secrets
        .into_iter()
        .map(|(id, secret)| (id, SecretString::from(secret)))
        .collect();
    match ctx.store.rekey(&supplied, "owner", now()) {
        Ok(()) => {
            changed(ctx, &[]);
            status_response(ctx)
        }
        Err(err) => fail(err),
    }
}

/// `GET /v1/vault/secrets`: the entries, without values.
pub(super) fn list(ctx: &Ctx) -> Response {
    match ctx.store.list() {
        Ok(entries) => Response::json(200, &entries),
        Err(err) => fail(err),
    }
}

#[derive(Deserialize)]
pub(super) struct CreateBody {
    name: String,
    kind: String,
    #[serde(default)]
    label: String,
    value: String,
    scope: Scope,
    #[serde(default)]
    bindings: Vec<String>,
}

/// `POST /v1/vault/secrets {name, kind, label?, value, scope, bindings}`.
pub(super) fn create(ctx: &Ctx, req: &Request) -> Response {
    let body: CreateBody = match parse(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if let Err(response) = check_nodes(ctx, &body.scope) {
        return response;
    }
    let new = NewSecret {
        name: body.name,
        kind: body.kind,
        label: body.label,
        value: SecretString::from(body.value),
        scope: body.scope,
        bindings: body.bindings,
    };
    match ctx.store.create(new, "owner", now()) {
        Ok(id) => {
            changed(ctx, std::slice::from_ref(&id));
            view_response(ctx, &id)
        }
        Err(err) => fail(err),
    }
}

#[derive(Deserialize)]
pub(super) struct ValueBody {
    value: String,
}

/// `PUT /v1/vault/secrets/{id}/value {value}`: a new version.
pub(super) fn rotate(ctx: &Ctx, id: &str, req: &Request) -> Response {
    let body: ValueBody = match parse(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    match ctx
        .store
        .rotate(id, &SecretString::from(body.value), "owner", now())
    {
        Ok(()) => {
            changed(ctx, &[id.to_string()]);
            view_response(ctx, id)
        }
        Err(err) => fail(err),
    }
}

#[derive(Deserialize)]
pub(super) struct PatchBody {
    label: Option<String>,
    scope: Option<Scope>,
    bindings: Option<Vec<String>>,
}

/// `PATCH /v1/vault/secrets/{id} {label?, scope?, bindings?}`.
pub(super) fn patch(ctx: &Ctx, id: &str, req: &Request) -> Response {
    let body: PatchBody = match parse(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if let Some(scope) = &body.scope
        && let Err(response) = check_nodes(ctx, scope)
    {
        return response;
    }
    match ctx
        .store
        .patch(id, body.label, body.scope, body.bindings, "owner", now())
    {
        Ok(()) => {
            changed(ctx, &[id.to_string()]);
            view_response(ctx, id)
        }
        Err(err) => fail(err),
    }
}

#[derive(Deserialize)]
pub(super) struct RollbackBody {
    v: u32,
}

/// `POST /v1/vault/secrets/{id}/rollback {v}`.
pub(super) fn rollback(ctx: &Ctx, id: &str, req: &Request) -> Response {
    let body: RollbackBody = match parse(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    match ctx.store.rollback(id, body.v, "owner", now()) {
        Ok(()) => {
            changed(ctx, &[id.to_string()]);
            view_response(ctx, id)
        }
        Err(err) => fail(err),
    }
}

/// `DELETE /v1/vault/secrets/{id}`: 204, and a tombstone.
pub(super) fn delete(ctx: &Ctx, id: &str) -> Response {
    match ctx.store.delete(id, "owner", now()) {
        Ok(()) => {
            changed(ctx, &[id.to_string()]);
            Response::new(204, "application/json", Vec::new())
        }
        Err(err) => fail(err),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct TestBody {
    node: Option<String>,
}

/// `POST /v1/vault/secrets/{id}/test {node?}`: the bound consumer's own check, with no value in the answer. The
/// resolution comes first: it proves the consumer reads the secret on that node. Then the consumer's harmless call:
/// the provider's key check, the connector's first read, or the Hugging Face account. An MCP server or a host tool
/// has no check of its own here.
pub(super) fn test(
    c: &Ctx,
    id: &str,
    req: &Request,
    probe: impl FnOnce(&str, &str) -> (bool, String),
) -> Response {
    let body: TestBody = match parse_or_default(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if let Some(node) = &body.node
        && let Err(response) = check_node(c, node)
    {
        return response;
    }
    match c.store.test(id, body.node, now()) {
        Ok((binding, node)) => {
            let (ok, detail) = probe(&binding, &node);
            Response::json(200, &json!({ "ok": ok, "detail": detail }))
        }
        Err(err) => fail(err),
    }
}

/// The probe, with every value the binding reads on `node` replaced by `•••` in its detail, so an echoed key never
/// leaves the route.
fn scrubbed(app: &App, binding: &str, node: &str) -> (bool, String) {
    let (ok, mut detail) = probe(app, binding, node);
    let owner = OnBehalf::Owner {
        node: Some(node.to_string()),
    };
    let names: Vec<String> = app
        .vault
        .list()
        .unwrap_or_default()
        .iter()
        .filter(|v| {
            v["bindings"]
                .as_array()
                .is_some_and(|b| b.iter().any(|x| x == binding))
        })
        .filter_map(|v| v["name"].as_str().map(str::to_string))
        .collect();
    for name in names {
        if let Some(value) = app.vault.value_for(&name, binding, &owner) {
            detail = detail.replace(&value, "•••");
        }
    }
    (ok, detail)
}

/// The bound consumer's own check, as `node` reads it: (ok, detail). Never a value.
fn probe(app: &App, binding: &str, node: &str) -> (bool, String) {
    let (kind, id) = binding.split_once(':').unwrap_or((binding, ""));
    match kind {
        "provider" => match app.providers.check_key(id, Some(node.to_string())) {
            Ok(detail) => (true, detail),
            Err(message) => (false, message),
        },
        "connector" => match app.connectors.check(id, Some(node.to_string())) {
            Ok(report) => (
                report["ok"] == true,
                report["detail"].as_str().unwrap_or_default().to_string(),
            ),
            Err((_, message)) => (false, message),
        },
        "hf" => {
            let block = app.settings.get().fetch_block;
            match app.hf.check_token(&app.vault, &block) {
                Ok(detail) => (true, detail),
                Err(message) => (false, message),
            }
        }
        "mcp" | "tool" => (
            false,
            "nothing to call here: start the server or use the tool to check it".into(),
        ),
        _ => (false, "no check for this binding".into()),
    }
}

#[derive(Deserialize)]
pub(super) struct ImportBody {
    format: String,
    text: String,
    scope: Scope,
    #[serde(default)]
    bindings: BTreeMap<String, Vec<String>>,
}

/// `POST /v1/vault/import {format:"dotenv", text, scope, bindings?}`.
pub(super) fn import(ctx: &Ctx, req: &Request) -> Response {
    let body: ImportBody = match parse(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.format != "dotenv" {
        return Response::error(400, "the import format is dotenv");
    }
    if let Err(response) = check_nodes(ctx, &body.scope) {
        return response;
    }
    match ctx
        .store
        .import_dotenv(&body.text, body.scope, &body.bindings, "owner", now())
    {
        Ok(report) => {
            let ids: Vec<String> = report.imported.iter().map(|(_, id)| id.clone()).collect();
            changed(ctx, &ids);
            Response::json(
                200,
                &json!({
                    "imported": report.imported.iter().map(|(name, id)| json!({ "name": name, "id": id })).collect::<Vec<_>>(),
                    "skipped": report.skipped.iter().map(|(name, reason)| json!({ "name": name, "reason": reason })).collect::<Vec<_>>(),
                }),
            )
        }
        Err(err) => fail(err),
    }
}

/// `GET /v1/vault/consumers`: each registered consumer and the entry each node resolves to.
pub(super) fn consumers(ctx: &Ctx) -> Response {
    match ctx.store.consumers(&nodes_of(ctx)) {
        Ok(list) => Response::json(200, &list),
        Err(err) => fail(err),
    }
}

/// `GET /v1/vault/audit?after=&limit=&verify=1`.
pub(super) fn audit(ctx: &Ctx, req: &Request) -> Response {
    let after = match query_number(req, "after", 0) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let limit = match query_number(req, "limit", 100) {
        Ok(value) => value.min(500) as usize,
        Err(response) => return response,
    };
    let entries = match ctx.store.audit_page(after, limit) {
        Ok(entries) => entries,
        Err(err) => return fail(err),
    };
    // `first_broken` and `verified` are null unless this request verified the chain.
    let (chain_ok, first_broken, verified): (Value, Value, Value) =
        if req.query_param("verify") == Some("1") {
            match ctx.store.check_audit() {
                Ok(verdict) => {
                    if let Some(line) = verdict.broken_at {
                        ctx.bus
                            .publish("vault.audit_broken", &json!({ "line": line }));
                    }
                    (
                        Value::from(verdict.ok),
                        json!(verdict.first_broken),
                        Value::from(verdict.verified),
                    )
                }
                Err(err) => return fail(err),
            }
        } else {
            (json!(ctx.store.audit_ok()), Value::Null, Value::Null)
        };
    Response::json(
        200,
        &json!({ "entries": entries, "chain_ok": chain_ok, "first_broken": first_broken, "verified": verified }),
    )
}

fn query_number(req: &Request, name: &str, default: u64) -> Result<u64, Response> {
    match req.query_param(name).filter(|v| !v.is_empty()) {
        None => Ok(default),
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| Response::error(400, &format!("{name} must be a number"))),
    }
}

/// A node the request names must be the hub's machine or a joined node that is not revoked.
fn check_node(ctx: &Ctx, node: &str) -> Result<(), Response> {
    if node == LOCAL_NODE || nodes_of(ctx).iter().any(|n| !n.revoked && n.id == node) {
        Ok(())
    } else {
        Err(Response::error(
            400,
            &format!("{node} is not a node of this plaza"),
        ))
    }
}

fn check_nodes(ctx: &Ctx, scope: &Scope) -> Result<(), Response> {
    match scope {
        Scope::Plaza => Ok(()),
        Scope::Nodes { nodes } => nodes.iter().try_for_each(|node| check_node(ctx, node)),
    }
}
