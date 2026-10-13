//! The Vault screen: the desktop's share of the web console's Vault view. Status, slots and entry
//! counts; when sealed, an unseal form (passphrase, recovery code or key file). When unsealed: seal,
//! the key slots (add a passphrase, remove one, make a recovery code), rekey, and the secrets (create,
//! rotate, roll back, test, remove), with a node selector that filters the list by scope.
//!
//! The hub never returns a secret value, and this screen never shows, logs or requests one. A value is
//! typed into a secure input and cleared from the state when its request is built. Every destructive
//! act asks first. The request bodies and the form state live in `vault_forms.rs`.

use std::collections::BTreeMap;
use std::fmt;

#[path = "vault_forms.rs"]
mod forms;
#[path = "vault_audit.rs"]
mod audit;

use audit::{AuditPage, AuditState, ImportForm, ImportResult};

use forms::{
    add_passphrase_body, create_body, create_card, pending_card, recovery_card, rekey_body, rekey_card, rollback_body, rotate_body,
    rotate_card, sealed_card, secret_path, slot_path, slots_card, test_body, unseal_body, PassForm, RekeyField, Rotate, ScopeMode,
    SecretForm, SecretKind, UnsealForm, UnsealMethod,
};
use iced::widget::{column, pick_list, row, text};
use iced::{Element, Length, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};

// ---- The hub's shapes (as apps/web/src/api.ts types them, trimmed to what this screen uses) ----

/// `GET /v1/vault`: the state, the slots and the counts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultStatus {
    /// `uninitialized`, `sealed`, `unsealed` or `tampered`.
    pub state: String,
    pub slots: Vec<VaultSlot>,
    pub entries: u64,
    pub audit_ok: bool,
    /// The reason the vault is tampered, when it is.
    pub tampered: Option<String>,
    /// The nodes the vault can scope to; `n0` is this hub.
    pub nodes: Vec<NodeRef>,
}

/// One key slot.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultSlot {
    pub id: String,
    /// `passphrase`, `keyfile` or `recovery`.
    pub kind: String,
    pub created_ms: f64,
}

/// The scope of an entry: the whole plaza, or the listed nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VaultScope {
    #[default]
    Plaza,
    Nodes { nodes: Vec<String> },
}

/// One version of an entry; only its number is shown (the rollback target).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VersionRef {
    pub v: u64,
}

/// One entry from `GET /v1/vault/secrets` (metadata only; the value is never returned).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultEntry {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub label: String,
    pub scope: VaultScope,
    pub bindings: Vec<String>,
    pub current: u64,
    pub versions: Vec<VersionRef>,
    pub last_used_ms: f64,
    pub use_count: u64,
}

/// A machine from `GET /v1/nodes`, for the selector and the scope names.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NodeRef {
    pub id: String,
    pub name: String,
}

/// One consumer from `GET /v1/vault/consumers`: a binding a secret can be bound to.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConsumerRef {
    pub binding: String,
    pub label: String,
    pub wants: String,
    /// Each node's state: an entry id, `missing`, or `out_of_scope`.
    pub per_node: BTreeMap<String, String>,
}

/// The answer of `POST /v1/vault/secrets/{id}/test`: no value in it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultTest {
    pub ok: bool,
    pub detail: String,
}

// ---- Filtering --------------------------------------------------------------------------------

/// What the entry list shows: everything, the plaza-scoped entries, or the entries a node is in scope for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Selection {
    #[default]
    All,
    Plaza,
    Node(String),
}

/// Whether an entry is shown under a selection. A node sees only the entries that list it.
pub fn in_selection(entry: &VaultEntry, selection: &Selection) -> bool {
    match (selection, &entry.scope) {
        (Selection::All, _) => true,
        (Selection::Plaza, VaultScope::Plaza) => true,
        (Selection::Plaza, VaultScope::Nodes { .. }) => false,
        (Selection::Node(id), VaultScope::Nodes { nodes }) => nodes.iter().any(|n| n == id),
        (Selection::Node(_), VaultScope::Plaza) => false,
    }
}

/// The entries a selection shows, in their loaded order.
pub fn filter_entries<'a>(entries: &'a [VaultEntry], selection: &Selection) -> Vec<&'a VaultEntry> {
    entries.iter().filter(|e| in_selection(e, selection)).collect()
}

/// One option of the selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub selection: Selection,
    pub label: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// All, Plaza, then each node.
fn choices(nodes: &[NodeRef]) -> Vec<Choice> {
    let mut out = vec![
        Choice { selection: Selection::All, label: "All".into() },
        Choice { selection: Selection::Plaza, label: "Plaza".into() },
    ];
    out.extend(nodes.iter().map(|n| Choice {
        selection: Selection::Node(n.id.clone()),
        label: if n.name.is_empty() { n.id.clone() } else { n.name.clone() },
    }));
    out
}

fn node_name(nodes: &[NodeRef], id: &str) -> String {
    nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| if n.name.is_empty() { n.id.clone() } else { n.name.clone() })
        .unwrap_or_else(|| id.to_string())
}

/// The version a rollback goes back to: the newest one before the current.
fn previous_version(entry: &VaultEntry) -> Option<u64> {
    entry.versions.iter().map(|v| v.v).filter(|v| *v < entry.current).max()
}

/// A slot's kind as the owner reads it.
fn slot_kind_label(kind: &str) -> String {
    match kind {
        "passphrase" => "Passphrase".into(),
        "keyfile" => "Key file".into(),
        "recovery" => "Recovery code".into(),
        other => other.to_string(),
    }
}

/// "never", or how long ago, from epoch milliseconds.
fn ago(ms: f64, now_ms: f64) -> String {
    if ms <= 0.0 {
        return "never".into();
    }
    let secs = ((now_ms - ms) / 1000.0).max(0.0) as u64;
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

// ---- State, messages, loading -----------------------------------------------------------------

/// The unsealed screen's sections: the secrets, then the consumers matrix, the audit log and the .env import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Secrets,
    Consumers,
    Audit,
    Import,
}

const TABS: [(Tab, &str); 4] = [(Tab::Secrets, "Secrets"), (Tab::Consumers, "Consumers"), (Tab::Audit, "Audit log"), (Tab::Import, ".env import")];

/// An act that asks for confirmation before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Seal,
    RemoveSlot { id: String, kind: String },
    Rekey,
    RemoveEntry { id: String, name: String },
    Rollback { id: String, name: String, v: u64 },
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// None until the first status answer.
    pub status: Option<VaultStatus>,
    /// Loaded only while the vault is unsealed.
    pub entries: Vec<VaultEntry>,
    pub nodes: Vec<NodeRef>,
    pub consumers: Vec<ConsumerRef>,
    pub selection: Selection,
    /// A load error, shown above the sections.
    pub error: Option<String>,
    /// A failed action, shown until the next one.
    pub failure: Option<String>,
    /// What the last action did.
    pub notice: Option<String>,
    /// One owner call is in flight; every action waits for it.
    pub busy: bool,
    /// The act waiting for its confirmation.
    pub pending: Option<Pending>,
    pub unseal: UnsealForm,
    pub add_pass: PassForm,
    /// A new recovery code, shown once until dismissed.
    pub recovery: Option<String>,
    /// The rekey form, open while the owner gives each slot's secret.
    pub rekey: Option<Vec<RekeyField>>,
    /// The create form, open while a secret is being added.
    pub create: Option<SecretForm>,
    /// The rotate form, open for one entry.
    pub rotate: Option<Rotate>,
    /// The last test's answer.
    pub test_note: Option<String>,
    /// The unsealed section on show.
    pub tab: Tab,
    /// The audit log, read one page at a time.
    pub audit: AuditState,
    /// The .env import, open until it is cleared or sent.
    pub import: ImportForm,
}

impl State {
    /// Drops every form and reveal that holds a secret; run whenever the vault is not unsealed.
    fn wipe(&mut self) {
        self.pending = None;
        self.add_pass = PassForm::default();
        self.recovery = None;
        self.rekey = None;
        self.create = None;
        self.rotate = None;
        self.import = ImportForm::default();
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Status(Result<VaultStatus, String>),
    Entries(Result<Vec<VaultEntry>, String>),
    Nodes(Result<Vec<NodeRef>, String>),
    Consumers(Result<Vec<ConsumerRef>, String>),
    Pick(Choice),
    /// An owner call's answer: the note to show, or the hub's error.
    Mutated(Result<String, String>),
    Created(Result<String, String>),
    Recovery(Result<String, String>),
    Tested(Result<VaultTest, String>),
    PickMethod(UnsealMethod),
    UnsealSecret(String),
    Unseal,
    Ask(Pending),
    Cancel,
    RunPending,
    PassPhrase(String),
    PassAgain(String),
    AddPassphrase,
    MakeRecovery,
    CopyRecovery,
    DismissRecovery,
    OpenRekey,
    RekeyInput(usize, String),
    CloseRekey,
    OpenCreate,
    CloseCreate,
    CName(String),
    CKind(SecretKind),
    CLabel(String),
    CValue(String),
    CScope(ScopeMode),
    CNode(usize, bool),
    CBinding(usize, bool),
    CSubmit,
    OpenRotate(String, String),
    RotateValue(String),
    RotateSubmit,
    CloseRotate,
    RunTest(String),
    Tab(Tab),
    AuditPage(Result<AuditPage, String>),
    AuditMore,
    AuditVerify,
    AuditVerified(Result<AuditPage, String>),
    ImportPick,
    ImportLoaded(Result<Option<(String, String)>, String>),
    ImportName(usize, bool),
    ImportNode(usize, bool),
    ImportBinding(usize, bool),
    ImportScope(ScopeMode),
    ImportClear,
    ImportSubmit,
    ImportDone(Result<ImportResult, String>),
    OpenCreateFor(String, String, String),
}

pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    Task::batch([fetch::<VaultStatus>(ctx, "/v1/vault", Msg::Status), fetch::<Vec<NodeRef>>(ctx, "/v1/nodes", Msg::Nodes)])
}

fn fetch<T>(ctx: &Ctx, path: &str, map: fn(Result<T, String>) -> Msg) -> Task<Msg>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let client = ctx.client.clone();
    let path = path.to_string();
    client::task(move || client.get::<T>(&path), map)
}

/// Runs one owner call in the background. Its answer comes back as `Mutated` with `note` on success.
fn call(state: &mut State, ctx: &Ctx, method: &'static str, path: String, body: Option<Value>, note: String) -> Task<Msg> {
    state.busy = true;
    state.failure = None;
    state.notice = None;
    let c = ctx.client.clone();
    client::task(move || c.call(method, &path, body.as_ref()).map(|_| note), Msg::Mutated)
}

/// The recovery code from `POST /v1/vault/recovery`.
fn recovery_code(v: &Value) -> Result<String, String> {
    v.get("recovery_code").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "The hub sent no recovery code.".to_string())
}

/// Applies an edit to the open create form, if there is one.
fn edit(state: &mut State, change: impl FnOnce(&mut SecretForm)) {
    if let Some(form) = state.create.as_mut() {
        change(form);
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Status(Ok(status)) => {
            state.error = None;
            let unsealed = status.state == "unsealed";
            state.status = Some(status);
            if !unsealed {
                state.wipe();
                state.entries.clear();
                state.consumers.clear();
                return Task::none();
            }
            Task::batch([
                fetch::<Vec<VaultEntry>>(ctx, "/v1/vault/secrets", Msg::Entries),
                fetch::<Vec<ConsumerRef>>(ctx, "/v1/vault/consumers", Msg::Consumers),
            ])
        }
        Msg::Status(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Entries(Ok(entries)) => {
            state.entries = entries;
            Task::none()
        }
        Msg::Entries(Err(error)) | Msg::Nodes(Err(error)) | Msg::Consumers(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Nodes(Ok(nodes)) => {
            state.nodes = nodes;
            Task::none()
        }
        Msg::Consumers(Ok(consumers)) => {
            state.consumers = consumers;
            Task::none()
        }
        Msg::Pick(choice) => {
            state.selection = choice.selection;
            Task::none()
        }

        // Owner calls and their answers.
        Msg::Mutated(Ok(note)) => {
            state.busy = false;
            state.pending = None;
            state.notice = Some(note);
            load(state, ctx)
        }
        Msg::Mutated(Err(error)) => {
            state.busy = false;
            state.pending = None;
            state.failure = Some(error);
            Task::none()
        }
        Msg::Created(Ok(note)) => {
            state.busy = false;
            state.create = None;
            state.notice = Some(note);
            load(state, ctx)
        }
        Msg::Created(Err(error)) => {
            state.busy = false;
            state.failure = Some(error);
            Task::none()
        }
        Msg::Recovery(Ok(code)) => {
            state.busy = false;
            state.recovery = Some(code);
            Task::none()
        }
        Msg::Recovery(Err(error)) => {
            state.busy = false;
            state.failure = Some(error);
            Task::none()
        }
        Msg::Tested(Ok(test)) => {
            state.test_note = Some(format!("{}: {}", if test.ok { "Test passed" } else { "Test failed" }, test.detail));
            Task::none()
        }
        Msg::Tested(Err(error)) => {
            state.failure = Some(error);
            Task::none()
        }

        // Unseal.
        Msg::PickMethod(method) => {
            state.unseal = UnsealForm { method, secret: String::new() };
            Task::none()
        }
        Msg::UnsealSecret(secret) => {
            state.unseal.secret = secret;
            Task::none()
        }
        Msg::Unseal => {
            let method = state.unseal.method;
            match unseal_body(method, &mut state.unseal.secret) {
                Ok(body) => call(state, ctx, "POST", "/v1/vault/unseal".into(), Some(body), "Vault unsealed".into()),
                Err(e) => {
                    state.failure = Some(e);
                    Task::none()
                }
            }
        }

        // Confirmations.
        Msg::Ask(pending) => {
            state.pending = Some(pending);
            Task::none()
        }
        Msg::Cancel => {
            state.pending = None;
            Task::none()
        }
        Msg::RunPending => match state.pending.take() {
            None => Task::none(),
            Some(Pending::Seal) => call(state, ctx, "POST", "/v1/vault/seal".into(), Some(json!({})), "Vault sealed".into()),
            Some(Pending::RemoveSlot { id, kind }) => {
                call(state, ctx, "DELETE", slot_path(&id), None, format!("Removed the {} slot", slot_kind_label(&kind).to_lowercase()))
            }
            Some(Pending::Rekey) => {
                let mut fields = state.rekey.take().unwrap_or_default();
                match rekey_body(&mut fields) {
                    Ok(body) => {
                        state.rekey = None;
                        call(state, ctx, "POST", "/v1/vault/rekey".into(), Some(body), "Vault rekeyed".into())
                    }
                    Err(e) => {
                        state.rekey = Some(fields);
                        state.failure = Some(e);
                        Task::none()
                    }
                }
            }
            Some(Pending::RemoveEntry { id, name }) => {
                call(state, ctx, "DELETE", secret_path(&id, ""), None, format!("Removed {name}"))
            }
            Some(Pending::Rollback { id, name, v }) => call(
                state,
                ctx,
                "POST",
                secret_path(&id, "/rollback"),
                Some(rollback_body(v)),
                format!("Rolled {name} back to v{v}"),
            ),
        },

        // Key slots and recovery.
        Msg::PassPhrase(s) => {
            state.add_pass.pass = s;
            Task::none()
        }
        Msg::PassAgain(s) => {
            state.add_pass.again = s;
            Task::none()
        }
        Msg::AddPassphrase => match add_passphrase_body(&mut state.add_pass) {
            Ok(body) => call(state, ctx, "POST", "/v1/vault/slots".into(), Some(body), "Passphrase slot added".into()),
            Err(e) => {
                state.failure = Some(e);
                Task::none()
            }
        },
        Msg::MakeRecovery => {
            state.busy = true;
            state.failure = None;
            state.notice = None;
            let c = ctx.client.clone();
            client::task(move || c.call("POST", "/v1/vault/recovery", Some(&json!({}))).and_then(|v| recovery_code(&v)), Msg::Recovery)
        }
        Msg::CopyRecovery => match &state.recovery {
            Some(code) => {
                state.notice = Some("Copied the recovery code.".into());
                iced::clipboard::write(code.clone())
            }
            None => Task::none(),
        },
        Msg::DismissRecovery => {
            state.recovery = None;
            Task::none()
        }

        // Rekey.
        Msg::OpenRekey => {
            if state.rekey.is_none() {
                state.rekey = state.status.as_ref().map(|s| {
                    s.slots
                        .iter()
                        .filter(|slot| slot.kind != "keyfile")
                        .map(|slot| RekeyField { slot: slot.id.clone(), kind: slot.kind.clone(), secret: String::new() })
                        .collect()
                });
            }
            Task::none()
        }
        Msg::RekeyInput(i, secret) => {
            if let Some(field) = state.rekey.as_mut().and_then(|fields| fields.get_mut(i)) {
                field.secret = secret;
            }
            Task::none()
        }
        Msg::CloseRekey => {
            state.rekey = None;
            if matches!(state.pending, Some(Pending::Rekey)) {
                state.pending = None;
            }
            Task::none()
        }

        // Create.
        Msg::OpenCreate => {
            state.tab = Tab::Secrets;
            state.create = Some(SecretForm {
                nodes: state.nodes.iter().map(|n| (n.id.clone(), false)).collect(),
                bindings: state.consumers.iter().map(|c| (c.binding.clone(), false)).collect(),
                ..Default::default()
            });
            Task::none()
        }
        Msg::CloseCreate => {
            state.create = None;
            Task::none()
        }
        Msg::CName(v) => {
            edit(state, |f| f.name = v);
            Task::none()
        }
        Msg::CKind(kind) => {
            edit(state, |f| f.kind = kind);
            Task::none()
        }
        Msg::CLabel(v) => {
            edit(state, |f| f.label = v);
            Task::none()
        }
        Msg::CValue(v) => {
            edit(state, |f| f.value = v);
            Task::none()
        }
        Msg::CScope(mode) => {
            edit(state, |f| f.scope = mode);
            Task::none()
        }
        Msg::CNode(i, on) => {
            edit(state, |f| {
                if let Some(node) = f.nodes.get_mut(i) {
                    node.1 = on;
                }
            });
            Task::none()
        }
        Msg::CBinding(i, on) => {
            edit(state, |f| {
                if let Some(binding) = f.bindings.get_mut(i) {
                    binding.1 = on;
                }
            });
            Task::none()
        }
        Msg::CSubmit => {
            let Some(form) = state.create.as_mut() else { return Task::none() };
            match create_body(form) {
                Ok(body) => {
                    let note = format!("Saved {}", body["name"].as_str().unwrap_or("the secret"));
                    state.busy = true;
                    state.failure = None;
                    state.notice = None;
                    let c = ctx.client.clone();
                    client::task(move || c.call("POST", "/v1/vault/secrets", Some(&body)).map(|_| note), Msg::Created)
                }
                Err(e) => {
                    state.failure = Some(e);
                    Task::none()
                }
            }
        }

        // Rotate (a new version of the value, through the one write route).
        Msg::OpenRotate(id, name) => {
            state.rotate = Some(Rotate { id, name, value: String::new() });
            Task::none()
        }
        Msg::RotateValue(v) => {
            if let Some(r) = state.rotate.as_mut() {
                r.value = v;
            }
            Task::none()
        }
        Msg::CloseRotate => {
            state.rotate = None;
            Task::none()
        }
        Msg::RotateSubmit => {
            let Some(r) = state.rotate.as_mut() else { return Task::none() };
            match rotate_body(&mut r.value) {
                Ok(body) => {
                    let path = secret_path(&r.id, "/value");
                    let note = format!("Rotated {}", r.name);
                    state.rotate = None;
                    call(state, ctx, "PUT", path, Some(body), note)
                }
                Err(e) => {
                    state.failure = Some(e);
                    Task::none()
                }
            }
        }

        // Test.
        Msg::RunTest(id) => {
            let node = match &state.selection {
                Selection::Node(n) => Some(n.clone()),
                _ => None,
            };
            state.test_note = None;
            let body = test_body(node.as_deref());
            let path = secret_path(&id, "/test");
            let c = ctx.client.clone();
            client::task(move || c.post::<VaultTest>(&path, &body), Msg::Tested)
        }

        // The consumers matrix, the audit log and the .env import.
        Msg::Tab(tab) => {
            state.tab = tab;
            if tab == Tab::Audit && !state.audit.loaded && !state.audit.busy {
                state.audit.busy = true;
                return audit::page_task(ctx, state.audit.after);
            }
            Task::none()
        }
        Msg::OpenCreateFor(name, binding, node) => {
            state.tab = Tab::Secrets;
            state.create = Some(SecretForm {
                name,
                scope: ScopeMode::Nodes,
                nodes: state.nodes.iter().map(|n| (n.id.clone(), n.id == node)).collect(),
                bindings: state.consumers.iter().map(|c| (c.binding.clone(), c.binding == binding)).collect(),
                ..Default::default()
            });
            Task::none()
        }
        Msg::AuditPage(result) => {
            state.audit.busy = false;
            match result {
                Ok(page) => {
                    state.audit.more = page.entries.len() as u64 == audit::PAGE;
                    if let Some(last) = page.entries.last() {
                        state.audit.after = last.seq;
                    }
                    state.audit.lines.extend(page.entries);
                    state.audit.loaded = true;
                    state.audit.error = None;
                }
                Err(error) => state.audit.error = Some(error),
            }
            Task::none()
        }
        Msg::AuditMore => {
            if state.audit.busy || !state.audit.more {
                return Task::none();
            }
            state.audit.busy = true;
            audit::page_task(ctx, state.audit.after)
        }
        Msg::AuditVerify => {
            if state.audit.busy {
                return Task::none();
            }
            state.audit.busy = true;
            audit::verify_task(ctx)
        }
        Msg::AuditVerified(Ok(page)) => {
            state.audit.busy = false;
            state.audit.error = None;
            state.audit.chain = Some(page.chain_ok.unwrap_or(false));
            Task::none()
        }
        Msg::AuditVerified(Err(error)) => {
            state.audit.busy = false;
            state.audit.error = Some(error);
            Task::none()
        }
        Msg::ImportPick => {
            state.failure = None;
            Task::perform(audit::pick_env_file(), Msg::ImportLoaded)
        }
        Msg::ImportLoaded(Ok(Some((name, raw)))) => {
            let (valid, invalid, skipped) = audit::scan_names(&raw);
            let scope = state.import.scope;
            state.import = ImportForm {
                source: Some(name),
                raw,
                picked: valid.iter().map(|n| (n.clone(), true)).collect(),
                bindings: state.consumers.iter().map(|c| (c.binding.clone(), valid.contains(&c.wants))).collect(),
                nodes: state.nodes.iter().map(|n| (n.id.clone(), false)).collect(),
                scope,
                invalid,
                skipped_lines: skipped,
                result: None,
            };
            Task::none()
        }
        Msg::ImportLoaded(Ok(None)) => Task::none(),
        Msg::ImportLoaded(Err(error)) => {
            state.failure = Some(error);
            Task::none()
        }
        Msg::ImportName(i, on) => {
            if let Some(p) = state.import.picked.get_mut(i) {
                p.1 = on;
            }
            Task::none()
        }
        Msg::ImportNode(i, on) => {
            if let Some(p) = state.import.nodes.get_mut(i) {
                p.1 = on;
            }
            Task::none()
        }
        Msg::ImportBinding(i, on) => {
            if let Some(p) = state.import.bindings.get_mut(i) {
                p.1 = on;
            }
            Task::none()
        }
        Msg::ImportScope(mode) => {
            state.import.scope = mode;
            Task::none()
        }
        Msg::ImportClear => {
            state.import = ImportForm::default();
            Task::none()
        }
        Msg::ImportSubmit => {
            let names: Vec<String> = state.import.picked.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect();
            if names.is_empty() {
                state.failure = Some("Pick at least one name to import.".into());
                return Task::none();
            }
            if state.import.raw.is_empty() {
                state.failure = Some("Choose the file again to retry.".into());
                return Task::none();
            }
            let scope = match audit::import_scope(&state.import) {
                Ok(scope) => scope,
                Err(error) => {
                    state.failure = Some(error);
                    return Task::none();
                }
            };
            let bindings = audit::import_bindings(state);
            let raw = std::mem::take(&mut state.import.raw);
            let body = audit::import_body(&raw, &names, scope, &bindings);
            drop(raw);
            state.import.picked.clear();
            state.import.invalid.clear();
            state.import.source = None;
            state.busy = true;
            state.failure = None;
            state.notice = None;
            let c = ctx.client.clone();
            client::task(move || c.post::<ImportResult>("/v1/vault/import", &body), Msg::ImportDone)
        }
        Msg::ImportDone(Ok(result)) => {
            state.busy = false;
            state.import.result = Some(result);
            load(state, ctx)
        }
        Msg::ImportDone(Err(error)) => {
            state.busy = false;
            state.failure = Some(format!("{error} The file was cleared, so choose it again to retry."));
            Task::none()
        }
    }
}

// ---- View -------------------------------------------------------------------------------------

/// A button that is inert while an owner call is in flight.
fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy {
        None
    } else {
        Some(msg)
    }
}

/// A column of elements with the screen's spacing.
fn stack<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    column(items).spacing(12).into()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::faint("Values are write-only: the vault never shows or returns a secret.").into()];
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if let Some(failure) = &state.failure {
        parts.push(ui::error_line(failure));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.clone()).into());
    }
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    match &state.status {
        None => parts.push(ui::loading()),
        Some(status) => {
            parts.push(header(status));
            match status.state.as_str() {
                "unsealed" => {
                    if state.create.is_none() {
                        actions.push(ui::btn("New secret", None, ui::Kind::Primary, gate(state.busy, Msg::OpenCreate)));
                    }
                    parts.extend(unsealed_section(state));
                }
                "sealed" => parts.push(sealed_card(state)),
                "tampered" => parts.push(ui::card(
                    "Tampered",
                    ui::error_line(status.tampered.as_deref().unwrap_or("The vault failed its integrity check.")),
                )),
                _ => parts.push(ui::card("Not set up", ui::faint("The vault has not been set up. Set it up in the web console.").into())),
            }
        }
    }
    ui::sheet(ui::view_head("Vault", actions), column(parts).spacing(16).into())
}

fn header<'a>(status: &VaultStatus) -> Element<'a, Msg> {
    let state = match status.state.as_str() {
        "unsealed" => "Unsealed",
        "sealed" => "Sealed",
        "uninitialized" => "Not set up",
        "tampered" => "Tampered",
        other => other,
    };
    let audit = if status.audit_ok { "audit ok" } else { "audit failed" };
    let line = format!("{state}  ·  {} key slots  ·  {} entries  ·  {audit}", status.slots.len(), status.entries);
    ui::card("Status", text(line).into())
}

/// The unsealed screen's cards, top to bottom: the confirmation, the recovery reveal, the key slots,
/// the rekey and rotate forms, the create form, and the secrets.
fn unsealed_parts<'a>(state: &'a State) -> Vec<Element<'a, Msg>> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(pending) = &state.pending {
        parts.push(pending_card(pending, state.busy));
    }
    if let Some(code) = &state.recovery {
        parts.push(recovery_card(code));
    }
    parts.push(slots_card(state));
    if let Some(fields) = &state.rekey {
        parts.push(rekey_card(fields, state.busy));
    }
    if let Some(form) = &state.create {
        parts.push(create_card(state, form));
    }
    if let Some(rotate) = &state.rotate {
        parts.push(rotate_card(rotate, state.busy));
    }
    parts.push(secrets_card(state));
    parts
}

/// The unsealed body under the section tabs: the secrets (the cards above), or one of the three sections.
fn unsealed_section<'a>(state: &'a State) -> Vec<Element<'a, Msg>> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::tabs(&TABS, state.tab, Msg::Tab)];
    match state.tab {
        Tab::Secrets => parts.extend(unsealed_parts(state)),
        Tab::Consumers => parts.push(audit::consumers_card(state)),
        Tab::Audit => parts.push(audit::audit_card(state)),
        Tab::Import => parts.push(audit::import_card(state)),
    }
    parts
}

fn selector<'a>(state: &'a State) -> Element<'a, Msg> {
    let options = choices(&state.nodes);
    let current = options.iter().find(|c| c.selection == state.selection).cloned().unwrap_or_else(|| options[0].clone());
    row(vec![ui::faint("Scope").into(), pick_list(options, Some(current), Msg::Pick).into()]).spacing(12).into()
}

/// The secrets: the scope selector, the test answer, and the entries with their row actions.
fn secrets_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let now = now_ms();
    let shown = filter_entries(&state.entries, &state.selection);
    let mut items: Vec<Element<'a, Msg>> = vec![selector(state)];
    if let Some(note) = &state.test_note {
        items.push(ui::faint(note.clone()).into());
    }
    if shown.is_empty() {
        items.push(ui::empty("key", "No entries", "Nothing in this scope yet."));
    }
    for (i, entry) in shown.iter().enumerate() {
        if i > 0 {
            items.push(ui::hairline());
        }
        items.push(entry_row(entry, state, now));
    }
    ui::card(&format!("Entries ({})", shown.len()), stack(items))
}

fn entry_row<'a>(entry: &VaultEntry, state: &'a State, now: f64) -> Element<'a, Msg> {
    let scope = match &entry.scope {
        VaultScope::Plaza => "Plaza".to_string(),
        VaultScope::Nodes { nodes: ids } => ids.iter().map(|id| node_name(&state.nodes, id)).collect::<Vec<_>>().join(", "),
    };
    let bindings = if entry.bindings.is_empty() { "no bindings".to_string() } else { entry.bindings.join(", ") };
    let name = if entry.label.is_empty() { entry.name.clone() } else { format!("{}  ({})", entry.name, entry.label) };
    let top: Element<'a, Msg> = row(vec![
        text(name).width(Length::FillPortion(3)).into(),
        text(entry.kind.clone()).width(Length::FillPortion(2)).into(),
        text(scope).width(Length::FillPortion(3)).into(),
        text(format!("v{}", entry.current)).width(Length::FillPortion(1)).into(),
    ])
    .spacing(12)
    .into();
    let detail = ui::faint(format!("Bindings: {bindings}  ·  used {} times, last {}", entry.use_count, ago(entry.last_used_ms, now)));
    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm("Rotate", None, ui::Kind::Secondary, gate(state.busy, Msg::OpenRotate(entry.id.clone(), entry.name.clone()))),
        ui::btn_sm("Test", None, ui::Kind::Secondary, gate(state.busy, Msg::RunTest(entry.id.clone()))),
    ];
    if let Some(v) = previous_version(entry) {
        let pending = Pending::Rollback { id: entry.id.clone(), name: entry.name.clone(), v };
        actions.push(ui::btn_sm(&format!("Roll back to v{v}"), None, ui::Kind::Secondary, gate(state.busy, Msg::Ask(pending))));
    }
    let remove = Pending::RemoveEntry { id: entry.id.clone(), name: entry.name.clone() };
    actions.push(ui::btn_sm("Remove", None, ui::Kind::Ghost, gate(state.busy, Msg::Ask(remove))));
    column(vec![top, detail.into(), row(actions).spacing(8).into(), ]).spacing(4).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, scope: VaultScope) -> VaultEntry {
        VaultEntry { name: name.into(), scope, ..Default::default() }
    }

    fn names(entries: Vec<&VaultEntry>) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn vault_filter_by_node_selection() {
        let entries = vec![
            entry("plaza-key", VaultScope::Plaza),
            entry("n0-only", VaultScope::Nodes { nodes: vec!["n0".into()] }),
            entry("both", VaultScope::Nodes { nodes: vec!["n0".into(), "n1".into()] }),
        ];
        assert_eq!(names(filter_entries(&entries, &Selection::All)), vec!["plaza-key", "n0-only", "both"]);
        assert_eq!(names(filter_entries(&entries, &Selection::Plaza)), vec!["plaza-key"]);
        assert_eq!(names(filter_entries(&entries, &Selection::Node("n0".into()))), vec!["n0-only", "both"]);
        assert_eq!(names(filter_entries(&entries, &Selection::Node("n1".into()))), vec!["both"]);
        assert!(filter_entries(&entries, &Selection::Node("n9".into())).is_empty());
    }

    #[test]
    fn vault_previous_version_is_the_newest_below_current() {
        let mut e = entry("K", VaultScope::Plaza);
        e.current = 3;
        e.versions = vec![VersionRef { v: 1 }, VersionRef { v: 2 }, VersionRef { v: 3 }];
        assert_eq!(previous_version(&e), Some(2));
        e.versions = vec![VersionRef { v: 3 }];
        assert_eq!(previous_version(&e), None);
    }

    #[test]
    fn vault_wipe_drops_every_secret_form() {
        let mut state = State {
            create: Some(SecretForm { value: "v".into(), ..Default::default() }),
            rotate: Some(Rotate { id: "a".into(), name: "A".into(), value: "v".into() }),
            recovery: Some("code".into()),
            add_pass: PassForm { pass: "p".into(), again: "p".into() },
            ..Default::default()
        };
        state.wipe();
        assert!(state.create.is_none() && state.rotate.is_none() && state.recovery.is_none());
        assert!(state.add_pass.pass.is_empty() && state.add_pass.again.is_empty());
    }
}
