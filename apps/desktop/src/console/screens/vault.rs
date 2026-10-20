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

#[path = "vault_audit.rs"]
mod audit;
#[path = "vault_forms.rs"]
mod forms;

use audit::{AuditPage, AuditState, ImportForm, ImportResult};

use forms::{
    EditForm, PassForm, RekeyField, Rotate, ScopeMode, SecretForm, SecretKind, UnsealForm,
    UnsealMethod, add_passphrase_body, create_body, edit_body, rekey_body, rollback_body,
    rotate_body, secret_path, slot_path, test_body, unseal_body,
};
use iced::widget::{Space, Stack, column, container, pick_list, row, text};
use iced::{Alignment, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, ui};
use crate::theme;

// ---- The hub's shapes (as apps/web/src/api.ts types them, trimmed to what this screen uses) ----

/// `GET /v1/vault`: the state, the slots and the counts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultStatus {
    /// `uninitialized`, `sealed`, `unsealed` or `tampered`.
    pub state: String,
    pub slots: Vec<VaultSlot>,
    pub entries: u64,
    /// The audit chain's last known state: None until the first unlock or audit check ("not checked").
    pub audit_ok: Option<bool>,
    /// When the vault was last rekeyed; None when never.
    pub rekeyed_ms: Option<f64>,
    /// How many variables the env file still holds; the banner shows while it is more than zero.
    pub env_file_keys: u64,
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
    /// True for the one slot that unlocked this session: a rekey does not ask for its secret.
    pub unsealed_with: bool,
}

/// The scope of an entry: the whole plaza, or the listed nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VaultScope {
    #[default]
    Plaza,
    Nodes {
        nodes: Vec<String>,
    },
}

/// One version of an entry: its number (the rollback target), when it was made, and the fingerprint and last four
/// characters the table shows. The value itself is never listed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VersionRef {
    pub v: u64,
    pub created_ms: f64,
    pub fp: String,
    pub hint: String,
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
    entries
        .iter()
        .filter(|e| in_selection(e, selection))
        .collect()
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
        Choice {
            selection: Selection::All,
            label: "All".into(),
        },
        Choice {
            selection: Selection::Plaza,
            label: "Plaza".into(),
        },
    ];
    out.extend(nodes.iter().map(|n| Choice {
        selection: Selection::Node(n.id.clone()),
        label: if n.name.is_empty() {
            n.id.clone()
        } else {
            n.name.clone()
        },
    }));
    out
}

fn node_name(nodes: &[NodeRef], id: &str) -> String {
    nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| {
            if n.name.is_empty() {
                n.id.clone()
            } else {
                n.name.clone()
            }
        })
        .unwrap_or_else(|| id.to_string())
}

/// The version a rollback goes back to: the newest one before the current.
fn previous_version(entry: &VaultEntry) -> Option<u64> {
    entry
        .versions
        .iter()
        .map(|v| v.v)
        .filter(|v| *v < entry.current)
        .max()
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

/// The rekey form's fields: one per passphrase or recovery slot, except the slot this session unlocked
/// with (the hub re-wraps that one from the session's own key). A key-file slot needs no secret either.
pub fn rekey_fields(status: &VaultStatus) -> Vec<RekeyField> {
    status
        .slots
        .iter()
        .filter(|slot| slot.kind != "keyfile" && !slot.unsealed_with)
        .map(|slot| RekeyField {
            slot: slot.id.clone(),
            kind: slot.kind.clone(),
            secret: String::new(),
        })
        .collect()
}

/// The audit chain's line, in the web console's words. None is "not checked", not "broken".
pub fn audit_line(audit_ok: Option<bool>) -> &'static str {
    match audit_ok {
        None => "Audit chain not checked until unsealed",
        Some(true) => "Audit chain intact",
        Some(false) => "Audit chain broken, see the Audit tab",
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

/// The unsealed screen's tabs, as the web's: the secrets, each entry's node matrix, and the audit log.
/// The .env import is a dialog from the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Secrets,
    Consumers,
    Audit,
}

const TABS: [(Tab, &str); 3] = [
    (Tab::Secrets, "Secrets"),
    (Tab::Consumers, "Per node"),
    (Tab::Audit, "Audit"),
];

/// This hub's own node id: the default node a test runs on.
const THIS_HUB: &str = "n0";

/// An act that asks for confirmation before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Seal,
    RemoveSlot {
        id: String,
        kind: String,
    },
    Rekey,
    RemoveEntry {
        id: String,
        name: String,
    },
    Rollback {
        id: String,
        name: String,
        v: u64,
    },
    /// Adds the key-file slot, once the owner has read what it does.
    AddKeyfile,
    /// Makes a new recovery code, which replaces the old one.
    Recovery,
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
    /// The unseal dialog, open while the vault is sealed and the owner unseals it.
    pub unseal_open: bool,
    pub unseal: UnsealForm,
    /// The add-passphrase dialog, open while a passphrase slot is being added.
    pub pass_open: bool,
    pub add_pass: PassForm,
    /// A new recovery code, shown once until dismissed.
    pub recovery: Option<String>,
    /// The owner has ticked "I saved it": the recovery dialog's close is enabled.
    pub recovery_saved: bool,
    /// The last copy did not reach the clipboard: the recovery dialog shows the code to select instead.
    pub copy_failed: bool,
    /// The rekey form, open while the owner gives each slot's secret.
    pub rekey: Option<Vec<RekeyField>>,
    /// The create form, open while a secret is being added.
    pub create: Option<SecretForm>,
    /// The rotate form, open for one entry.
    pub rotate: Option<Rotate>,
    /// The edit form, open for one entry: its label, scope and bindings (no value).
    pub edit: Option<EditForm>,
    /// The test dialog, open for one entry: its id and name.
    pub test: Option<(String, String)>,
    /// The node the test runs on (`n0` is this hub).
    pub test_node: String,
    /// The last test's answer, shown in the test dialog.
    pub test_note: Option<VaultTest>,
    /// The unsealed section on show.
    pub tab: Tab,
    /// The audit log, read one page at a time.
    pub audit: AuditState,
    /// The .env import dialog, open until it is closed.
    pub import_open: bool,
    /// The .env import, held until it is cleared or sent.
    pub import: ImportForm,
}

impl State {
    /// Drops every form and reveal that holds a secret; run whenever the vault is not unsealed.
    /// The unseal dialog stays: it is what unseals the vault.
    fn wipe(&mut self) {
        self.pending = None;
        self.pass_open = false;
        self.add_pass = PassForm::default();
        self.recovery = None;
        self.recovery_saved = false;
        self.rekey = None;
        self.create = None;
        self.rotate = None;
        self.edit = None;
        self.test = None;
        self.test_note = None;
        self.import_open = false;
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
    /// The clipboard's read-back after a copy: true when the code is on it.
    CopyChecked(bool),
    /// Edits to the read-only recovery field; they are dropped.
    Noop,
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
    /// Adds the key-file slot (the hub writes the key file on its own disk).
    AddKeyfile,
    OpenEdit(String),
    EditLabel(String),
    EditScope(ScopeMode),
    EditNode(usize, bool),
    EditBinding(usize, bool),
    EditSubmit,
    CloseEdit,
    Edited(Result<String, String>),
    /// Opens or closes the unseal dialog.
    OpenUnseal,
    CloseUnseal,
    /// Unseals with the key file (no secret is typed).
    UnsealKeyfile,
    /// Opens or closes the add-passphrase dialog.
    OpenPass,
    ClosePass,
    /// The recovery dialog's "I saved it" tick.
    RecoverySaved(bool),
    /// Opens the test dialog for one entry, and closes it.
    OpenTest(String, String),
    CloseTest,
    /// The node a test runs on.
    TestNode(String),
    /// Opens or closes the .env import dialog.
    OpenImport,
    CloseImport,
}

pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    Task::batch([
        fetch::<VaultStatus>(ctx, "/v1/vault", Msg::Status),
        fetch::<Vec<NodeRef>>(ctx, "/v1/nodes", Msg::Nodes),
    ])
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
fn call(
    state: &mut State,
    ctx: &Ctx,
    method: &'static str,
    path: String,
    body: Option<Value>,
    note: String,
) -> Task<Msg> {
    state.busy = true;
    state.failure = None;
    state.notice = None;
    let c = ctx.client.clone();
    client::task(
        move || c.call(method, &path, body.as_ref()).map(|_| note),
        Msg::Mutated,
    )
}

/// The recovery code from `POST /v1/vault/recovery`.
fn recovery_code(v: &Value) -> Result<String, String> {
    v.get("recovery_code")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "The hub sent no recovery code.".to_string())
}

/// Applies an edit to the open create form, if there is one.
fn edit(state: &mut State, change: impl FnOnce(&mut SecretForm)) {
    if let Some(form) = state.create.as_mut() {
        change(form);
    }
}

/// Applies an edit to the open edit form, if there is one.
fn amend(state: &mut State, change: impl FnOnce(&mut EditForm)) {
    if let Some(form) = state.edit.as_mut() {
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
            // Unsealed: the unseal dialog has done its job.
            state.unseal_open = false;
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
            state.unseal_open = false;
            state.pass_open = false;
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
            state.recovery_saved = false;
            Task::none()
        }
        Msg::Recovery(Err(error)) => {
            state.busy = false;
            state.failure = Some(error);
            Task::none()
        }
        Msg::Tested(Ok(test)) => {
            state.test_note = Some(test);
            Task::none()
        }
        Msg::Tested(Err(error)) => {
            state.failure = Some(error);
            Task::none()
        }

        // Unseal.
        Msg::PickMethod(method) => {
            state.unseal = UnsealForm {
                method,
                secret: String::new(),
            };
            Task::none()
        }
        Msg::UnsealSecret(secret) => {
            state.unseal.secret = secret;
            Task::none()
        }
        Msg::Unseal => unseal(state, ctx),
        Msg::UnsealKeyfile => {
            state.unseal = UnsealForm {
                method: UnsealMethod::KeyFile,
                secret: String::new(),
            };
            unseal(state, ctx)
        }
        Msg::OpenUnseal => {
            state.unseal_open = true;
            state.failure = None;
            Task::none()
        }
        Msg::CloseUnseal => {
            state.unseal_open = false;
            state.unseal.secret.clear();
            state.failure = None;
            Task::none()
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
            Some(Pending::Seal) => call(
                state,
                ctx,
                "POST",
                "/v1/vault/seal".into(),
                Some(json!({})),
                "Vault sealed".into(),
            ),
            Some(Pending::RemoveSlot { id, kind }) => call(
                state,
                ctx,
                "DELETE",
                slot_path(&id),
                None,
                format!("Removed the {} slot", slot_kind_label(&kind).to_lowercase()),
            ),
            Some(Pending::Rekey) => {
                let mut fields = state.rekey.take().unwrap_or_default();
                match rekey_body(&mut fields) {
                    Ok(body) => {
                        state.rekey = None;
                        call(
                            state,
                            ctx,
                            "POST",
                            "/v1/vault/rekey".into(),
                            Some(body),
                            "Vault rekeyed".into(),
                        )
                    }
                    Err(e) => {
                        state.rekey = Some(fields);
                        state.failure = Some(e);
                        Task::none()
                    }
                }
            }
            Some(Pending::RemoveEntry { id, name }) => call(
                state,
                ctx,
                "DELETE",
                secret_path(&id, ""),
                None,
                format!("Removed {name}"),
            ),
            Some(Pending::Rollback { id, name, v }) => call(
                state,
                ctx,
                "POST",
                secret_path(&id, "/rollback"),
                Some(rollback_body(v)),
                format!("Rolled {name} back to v{v}"),
            ),
            Some(Pending::AddKeyfile) => update(state, Msg::AddKeyfile, ctx),
            Some(Pending::Recovery) => update(state, Msg::MakeRecovery, ctx),
        },

        // Key slots and recovery.
        Msg::OpenPass => {
            state.pass_open = true;
            state.failure = None;
            Task::none()
        }
        Msg::ClosePass => {
            state.pass_open = false;
            state.add_pass = PassForm::default();
            state.failure = None;
            Task::none()
        }
        Msg::PassPhrase(s) => {
            state.add_pass.pass = s;
            Task::none()
        }
        Msg::PassAgain(s) => {
            state.add_pass.again = s;
            Task::none()
        }
        Msg::AddPassphrase => match add_passphrase_body(&mut state.add_pass) {
            Ok(body) => call(
                state,
                ctx,
                "POST",
                "/v1/vault/slots".into(),
                Some(body),
                "Passphrase slot added".into(),
            ),
            Err(e) => {
                state.failure = Some(e);
                Task::none()
            }
        },
        Msg::AddKeyfile => call(
            state,
            ctx,
            "POST",
            "/v1/vault/slots".into(),
            Some(json!({ "kind": "keyfile" })),
            "Key file slot added".into(),
        ),
        Msg::MakeRecovery => {
            state.busy = true;
            state.copy_failed = false;
            state.failure = None;
            state.notice = None;
            let c = ctx.client.clone();
            client::task(
                move || {
                    c.call("POST", "/v1/vault/recovery", Some(&json!({})))
                        .and_then(|v| recovery_code(&v))
                },
                Msg::Recovery,
            )
        }
        Msg::CopyRecovery => {
            match &state.recovery {
                // iced's clipboard write cannot report failure, so the code is read back: a copy that did not
                // land (no clipboard) reads back as nothing, and the card then shows the code to select.
                Some(code) => {
                    let expected = code.clone();
                    iced::clipboard::write::<Msg>(code.clone()).chain(iced::clipboard::read().map(
                        move |got| Msg::CopyChecked(got.as_deref() == Some(expected.as_str())),
                    ))
                }
                None => Task::none(),
            }
        }
        Msg::CopyChecked(ok) => {
            state.copy_failed = !ok;
            state.notice = if ok {
                Some("Copied the recovery code.".into())
            } else {
                None
            };
            Task::none()
        }
        Msg::Noop => Task::none(),
        Msg::DismissRecovery => {
            state.recovery = None;
            state.recovery_saved = false;
            Task::none()
        }
        Msg::RecoverySaved(on) => {
            state.recovery_saved = on;
            Task::none()
        }

        // Rekey.
        Msg::OpenRekey => {
            if state.rekey.is_none() {
                state.rekey = state.status.as_ref().map(rekey_fields);
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
                bindings: state
                    .consumers
                    .iter()
                    .map(|c| (c.binding.clone(), false))
                    .collect(),
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
            let Some(form) = state.create.as_mut() else {
                return Task::none();
            };
            match create_body(form) {
                Ok(body) => {
                    let note = format!("Saved {}", body["name"].as_str().unwrap_or("the secret"));
                    state.busy = true;
                    state.failure = None;
                    state.notice = None;
                    let c = ctx.client.clone();
                    client::task(
                        move || {
                            c.call("POST", "/v1/vault/secrets", Some(&body))
                                .map(|_| note)
                        },
                        Msg::Created,
                    )
                }
                Err(e) => {
                    state.failure = Some(e);
                    Task::none()
                }
            }
        }

        // Rotate (a new version of the value, through the one write route).
        Msg::OpenRotate(id, name) => {
            state.rotate = Some(Rotate {
                id,
                name,
                value: String::new(),
            });
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
            let Some(r) = state.rotate.as_mut() else {
                return Task::none();
            };
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

        // Edit: the label, scope and bindings (PATCH); the value is not part of it.
        Msg::OpenEdit(id) => {
            let Some(entry) = state.entries.iter().find(|e| e.id == id) else {
                return Task::none();
            };
            let scoped: Vec<String> = match &entry.scope {
                VaultScope::Nodes { nodes } => nodes.clone(),
                VaultScope::Plaza => Vec::new(),
            };
            // The entry's current bindings stay ticked even when no consumer lists them.
            let mut bindings: Vec<(String, bool)> = state
                .consumers
                .iter()
                .map(|c| (c.binding.clone(), entry.bindings.contains(&c.binding)))
                .collect();
            bindings.extend(
                entry
                    .bindings
                    .iter()
                    .filter(|b| !state.consumers.iter().any(|c| &c.binding == *b))
                    .map(|b| (b.clone(), true)),
            );
            state.edit = Some(EditForm {
                id: entry.id.clone(),
                name: entry.name.clone(),
                label: entry.label.clone(),
                scope: if matches!(entry.scope, VaultScope::Nodes { .. }) {
                    ScopeMode::Nodes
                } else {
                    ScopeMode::Plaza
                },
                nodes: state
                    .nodes
                    .iter()
                    .map(|n| (n.id.clone(), scoped.contains(&n.id)))
                    .collect(),
                bindings,
            });
            Task::none()
        }
        Msg::EditLabel(v) => {
            amend(state, |f| f.label = v);
            Task::none()
        }
        Msg::EditScope(mode) => {
            amend(state, |f| f.scope = mode);
            Task::none()
        }
        Msg::EditNode(i, on) => {
            amend(state, |f| {
                if let Some(node) = f.nodes.get_mut(i) {
                    node.1 = on;
                }
            });
            Task::none()
        }
        Msg::EditBinding(i, on) => {
            amend(state, |f| {
                if let Some(binding) = f.bindings.get_mut(i) {
                    binding.1 = on;
                }
            });
            Task::none()
        }
        Msg::CloseEdit => {
            state.edit = None;
            Task::none()
        }
        Msg::EditSubmit => {
            let Some(form) = state.edit.as_ref() else {
                return Task::none();
            };
            match edit_body(form) {
                Ok(body) => {
                    let path = secret_path(&form.id, "");
                    let note = format!("Saved {}", form.name);
                    state.busy = true;
                    state.failure = None;
                    state.notice = None;
                    let c = ctx.client.clone();
                    client::task(
                        move || c.call("PATCH", &path, Some(&body)).map(|_| note),
                        Msg::Edited,
                    )
                }
                Err(e) => {
                    state.failure = Some(e);
                    Task::none()
                }
            }
        }
        // The form stays open on a refusal, so the ticks are not lost.
        Msg::Edited(Ok(note)) => {
            state.busy = false;
            state.edit = None;
            state.notice = Some(note);
            load(state, ctx)
        }
        Msg::Edited(Err(error)) => {
            state.busy = false;
            state.failure = Some(error);
            Task::none()
        }

        // Test.
        Msg::OpenTest(id, name) => {
            state.test = Some((id, name));
            state.test_node = THIS_HUB.to_string();
            state.test_note = None;
            state.failure = None;
            Task::none()
        }
        Msg::CloseTest => {
            state.test = None;
            state.test_note = None;
            state.failure = None;
            Task::none()
        }
        Msg::TestNode(node) => {
            state.test_node = node;
            Task::none()
        }
        Msg::RunTest(id) => {
            let node = Some(state.test_node.clone()).filter(|n| !n.is_empty());
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
                nodes: state
                    .nodes
                    .iter()
                    .map(|n| (n.id.clone(), n.id == node))
                    .collect(),
                bindings: state
                    .consumers
                    .iter()
                    .map(|c| (c.binding.clone(), c.binding == binding))
                    .collect(),
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
            state.audit.broken = page.first_broken.clone();
            state.audit.verified = page.verified;
            Task::none()
        }
        Msg::AuditVerified(Err(error)) => {
            state.audit.busy = false;
            state.audit.error = Some(error);
            Task::none()
        }
        Msg::OpenImport => {
            state.import_open = true;
            state.failure = None;
            Task::none()
        }
        Msg::CloseImport => {
            state.import_open = false;
            state.import = ImportForm::default();
            state.failure = None;
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
                bindings: state
                    .consumers
                    .iter()
                    .map(|c| (c.binding.clone(), valid.contains(&c.wants)))
                    .collect(),
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
            let names: Vec<String> = state
                .import
                .picked
                .iter()
                .filter(|(_, on)| *on)
                .map(|(n, _)| n.clone())
                .collect();
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
            client::task(
                move || c.post::<ImportResult>("/v1/vault/import", &body),
                Msg::ImportDone,
            )
        }
        Msg::ImportDone(Ok(result)) => {
            state.busy = false;
            state.import.result = Some(result);
            load(state, ctx)
        }
        Msg::ImportDone(Err(error)) => {
            state.busy = false;
            state.failure = Some(format!(
                "{error} The file was cleared, so choose it again to retry."
            ));
            Task::none()
        }
    }
}

// ---- View -------------------------------------------------------------------------------------

/// A button that is inert while an owner call is in flight.
/// The message a control sends while no owner call is in flight; None disables it.
fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

/// Unseals with the form's method: the one secret it holds, or the key file (no secret).
fn unseal(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let method = state.unseal.method;
    match unseal_body(method, &mut state.unseal.secret) {
        Ok(body) => call(
            state,
            ctx,
            "POST",
            "/v1/vault/unseal".into(),
            Some(body),
            "Vault unsealed".into(),
        ),
        Err(e) => {
            state.failure = Some(e);
            Task::none()
        }
    }
}

/// The version the entry resolves to now (the last one when the number is missing).
fn current_version(entry: &VaultEntry) -> Option<&VersionRef> {
    entry
        .versions
        .iter()
        .find(|v| v.v == entry.current)
        .or_else(|| entry.versions.last())
}

/// A binding's label as the web reads it: the consumer's label when it is known, else a plain reading of its name.
fn binding_name(binding: &str, consumers: &[ConsumerRef]) -> String {
    if let Some(c) = consumers.iter().find(|c| c.binding == binding) {
        return c.label.clone();
    }
    match binding.split_once(':') {
        Some(("provider", rest)) => format!("{rest} provider"),
        Some(("mcp", rest)) => format!("MCP {rest}"),
        Some(("connector", rest)) => format!("{rest} connector"),
        Some(("tool", rest)) => format!("tool {rest}"),
        _ if binding == "hf" => "Hugging Face".to_string(),
        _ => binding.to_string(),
    }
}

/// A form field, as the web's: its label in the label voice, the control, then a hint line when there is one.
pub(super) fn field<'a>(
    label: &str,
    control: Element<'a, Msg>,
    hint: Option<&str>,
) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), control];
    if let Some(hint) = hint {
        items.push(ui::faint(hint.to_string()).into());
    }
    column(items).spacing(6).width(Length::Fill).into()
}

/// The open dialog's own failure line. With no dialog open, the page shows the failure instead.
pub(super) fn failure_line<'a>(state: &State) -> Option<Element<'a, Msg>> {
    state.failure.as_deref().map(ui::error_line)
}

/// A rule the width of a card, in the strong ink-rule tone (under a table's head).
fn strong_rule<'a>() -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule_strong.into()),
            ..Default::default()
        })
        .into()
}

/// The web's table: a head row in the label voice over a strong rule, then each row under a faint rule.
/// Columns are proportional; each row is built with `cells`.
pub(super) fn table<'a>(head: &[(u16, &str)], rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let heads: Vec<(u16, Element<'a, Msg>)> = head
        .iter()
        .map(|(portion, label)| (*portion, ui::label(label).into()))
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![cells(heads), strong_rule()];
    for line in rows {
        lines.push(line);
        lines.push(ui::hairline());
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: its cells in proportional columns, padded 9 by 12 as the web's cells are.
pub(super) fn cells<'a>(parts: Vec<(u16, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let laid: Vec<Element<'a, Msg>> = parts
        .into_iter()
        .map(|(portion, cell)| container(cell).width(Length::FillPortion(portion)).into())
        .collect();
    container(row(laid).spacing(12).align_y(Alignment::Center))
        .padding([9, 12])
        .width(Length::Fill)
        .into()
}

/// A card whose body is flush (a table or a list under its head, with no padding of its own), as the web's
/// `.card` with its flush body. The head is the card's own: its title in the label voice, its actions at the right.
pub(super) fn flush_card<'a>(
    title: &str,
    actions: Vec<Element<'a, Msg>>,
    body: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let mut head = row![ui::card_title(title)]
        .spacing(8)
        .align_y(Alignment::Center);
    head = head.push(Space::new().width(Length::Fill));
    for action in actions {
        head = head.push(action);
    }
    let head = container(head)
        .padding([8, 16])
        .width(Length::Fill)
        .center_y(Length::Fixed(44.0));
    container(column![head, strong_rule(), body])
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// A dialog over the screen, as the web's modal: a sheet on a scrim, with its title, its body (which scrolls past
/// a height) and its actions at the right. The caller stacks it over the page.
pub(super) fn layer<'a>(
    title: &str,
    body: Vec<Element<'a, Msg>>,
    actions: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut foot: Vec<Element<'a, Msg>> = vec![Space::new().width(Length::Fill).into()];
    foot.extend(actions);
    let sheet = container(
        column![
            text(title.to_string())
                .font(theme::SANS_SEMIBOLD)
                .size(theme::TYPE_SECTION)
                .color(theme::palette().ink),
            container(
                ui::scroll(column(body).spacing(12).width(Length::Fill))
                    .height(Length::Shrink)
                    .width(Length::Fill)
            )
            .max_height(480.0),
            row(foot).spacing(8).align_y(Alignment::Center),
        ]
        .spacing(16)
        .width(Length::Fill),
    )
    .padding(20)
    .width(Length::Fixed(520.0))
    .style(theme::pop);
    container(sheet)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(theme::scrim)
        .into()
}

/// The state's label, badge tone and line, as the web's status card words them.
fn state_label(state: &str) -> &'static str {
    match state {
        "unsealed" => "Unsealed",
        "sealed" => "Sealed",
        "tampered" => "Tampered",
        _ => "Uninitialized",
    }
}

fn state_tone(state: &str) -> ui::Tone {
    match state {
        "unsealed" => ui::Tone::Ok,
        "sealed" => ui::Tone::Warn,
        "tampered" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

fn state_copy(status: &VaultStatus) -> String {
    match status.state.as_str() {
        "unsealed" => "Secrets resolve for agents, connectors and MCP servers.".to_string(),
        "sealed" => "Secrets stay encrypted. Unseal to use them.".to_string(),
        "tampered" => format!(
            "{} Secrets are not available. Check the audit log.",
            status
                .tampered
                .as_deref()
                .unwrap_or("The vault failed an integrity check.")
        ),
        _ => {
            "The vault is not set up. It is created when apid starts with a vault key.".to_string()
        }
    }
}

/// The status card: the state and its line, then the key slots (each removable), the secret count, when the vault
/// was last rekeyed and the audit chain. Its actions follow the state: the key work when unsealed, Unseal when sealed.
fn status_card<'a>(state: &'a State, status: &'a VaultStatus) -> Element<'a, Msg> {
    let unsealed = status.state == "unsealed";
    let has_keyfile = status.slots.iter().any(|s| s.kind == "keyfile");
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if unsealed {
        let keyfile = if has_keyfile {
            None
        } else {
            gate(state.busy, Msg::Ask(Pending::AddKeyfile))
        };
        actions.push(ui::btn(
            "Seal",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::Ask(Pending::Seal)),
        ));
        actions.push(ui::btn(
            "Add passphrase",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::OpenPass),
        ));
        actions.push(ui::btn(
            "Add key file slot",
            None,
            ui::Kind::Secondary,
            keyfile,
        ));
        actions.push(ui::btn(
            "Create recovery code",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::Ask(Pending::Recovery)),
        ));
        actions.push(ui::btn(
            "Rekey",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::OpenRekey),
        ));
    } else if matches!(status.state.as_str(), "sealed" | "tampered") {
        actions.push(ui::btn(
            "Unseal",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::OpenUnseal),
        ));
    }

    let now = now_ms();
    let count = status.slots.len();
    let slot_rows: Vec<Element<'a, Msg>> = status
        .slots
        .iter()
        .map(|slot| {
            let mut parts: Vec<Element<'a, Msg>> = vec![
                text(slot_kind_label(&slot.kind)).into(),
                ui::faint(format!("Added {}", ago(slot.created_ms, now))).into(),
            ];
            if unsealed {
                // The hub keeps at least one slot, so the last one is not removable.
                let remove = if count > 1 {
                    gate(
                        state.busy,
                        Msg::Ask(Pending::RemoveSlot {
                            id: slot.id.clone(),
                            kind: slot.kind.clone(),
                        }),
                    )
                } else {
                    None
                };
                parts.push(ui::btn_sm("Remove", None, ui::Kind::Ghost, remove));
            }
            row(parts).spacing(12).align_y(Alignment::Center).into()
        })
        .collect();
    let slots: Element<'a, Msg> = if slot_rows.is_empty() {
        ui::faint("None").into()
    } else {
        column(slot_rows).spacing(6).into()
    };
    let rekeyed = match status.rekeyed_ms {
        Some(ms) => ago(ms, now),
        None => "Never".to_string(),
    };
    let facts = row![
        fact_cell("Key slots", slots),
        fact_cell(
            "Secrets",
            ui::sans(status.entries.to_string(), 13.0, theme::palette().ink).into()
        ),
        fact_cell(
            "Rekeyed",
            ui::sans(rekeyed, 13.0, theme::palette().ink).into()
        ),
        fact_cell("Audit", audit_badge(status.audit_ok)),
    ]
    .spacing(24);
    let top = row![
        ui::badge_dot(state_label(&status.state), state_tone(&status.state)),
        ui::faint(state_copy(status)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    ui::card_with(
        "Status",
        actions,
        column![top, facts].spacing(16).width(Length::Fill).into(),
    )
}

/// One fact of the status card: its label above its value.
fn fact_cell<'a>(label: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(label), value]
        .spacing(6)
        .width(Length::FillPortion(1))
        .into()
}

/// The audit chain's badge: not checked until unsealed, then intact or broken.
fn audit_badge<'a>(ok: Option<bool>) -> Element<'a, Msg> {
    let tone = match ok {
        None => ui::Tone::Muted,
        Some(true) => ui::Tone::Ok,
        Some(false) => ui::Tone::Danger,
    };
    ui::badge(audit_line(ok), tone)
}

/// The env-file banner, as the web's: keys that were in an env file now live in the vault.
fn env_banner<'a>() -> Element<'a, Msg> {
    let p = theme::palette();
    let warn_soft = p.warn_soft;
    container(
        column![
            text("Keys were found in an env file. They now live in the vault. Delete the file.")
                .font(theme::SANS_SEMIBOLD)
                .size(theme::TYPE_UI)
                .color(p.ink),
            row![
                ui::faint("To move what is left and remove the file in one step, run "),
                ui::mono("apz vault import FILE.env --shred", 12.0, p.ink),
                ui::faint("."),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(4),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(move |_: &Theme| container::Style {
        background: Some(warn_soft.into()),
        border: iced::Border::default().rounded(theme::skin().radius),
        ..Default::default()
    })
    .into()
}

/// The toolbar: Add secret and Import .env. Both are off while the vault is locked.
fn toolbar<'a>(state: &'a State, locked: bool) -> Element<'a, Msg> {
    let add = if locked {
        None
    } else {
        gate(state.busy, Msg::OpenCreate)
    };
    let import = if locked {
        None
    } else {
        gate(state.busy, Msg::OpenImport)
    };
    row![
        ui::btn("Add secret", None, ui::Kind::Primary, add),
        ui::btn("Import .env", None, ui::Kind::Ghost, import),
    ]
    .spacing(8)
    .into()
}

/// The tab's content: the secrets, the node matrix, or the audit log.
fn tab_content<'a>(state: &'a State, locked: bool) -> Vec<Element<'a, Msg>> {
    match state.tab {
        Tab::Secrets => secrets_section(state, locked),
        Tab::Consumers => vec![audit::consumers_card(state)],
        Tab::Audit => vec![audit::audit_card(state)],
    }
}

/// The Secrets tab: the scope field, then the list (or its empty state) in a flush card.
fn secrets_section<'a>(state: &'a State, locked: bool) -> Vec<Element<'a, Msg>> {
    if locked {
        return vec![ui::card(
            "Secrets",
            ui::empty(
                "key",
                "Vault is locked",
                "Unseal the vault to see and manage secrets.",
            ),
        )];
    }
    if state.entries.is_empty() {
        return vec![ui::card(
            "Secrets",
            ui::empty(
                "key",
                "No secrets yet",
                "Add one with Add secret, or import a .env file.",
            ),
        )];
    }
    let options = choices(&state.nodes);
    let current = options
        .iter()
        .find(|c| c.selection == state.selection)
        .cloned()
        .unwrap_or_else(|| options[0].clone());
    let scope = field(
        "Scope",
        pick_list(options, Some(current), Msg::Pick)
            .style(crate::theme::pick_list)
            .into(),
        None,
    );
    let shown = filter_entries(&state.entries, &state.selection);
    if shown.is_empty() {
        return vec![
            scope,
            ui::card(
                "Secrets",
                ui::empty(
                    "key",
                    "No secrets in this scope",
                    "Pick another scope above.",
                ),
            ),
        ];
    }
    let rows: Vec<Element<'a, Msg>> = shown
        .into_iter()
        .map(|entry| entry_row(entry, state))
        .collect();
    let head: [(u16, &str); 10] = [
        (3, "Name"),
        (2, "Kind"),
        (2, "Scope"),
        (3, "Bindings"),
        (2, "Fingerprint"),
        (1, "Last 4"),
        (1, "Version"),
        (2, "Last used"),
        (1, "Uses"),
        (4, "Actions"),
    ];
    vec![scope, flush_card("Secrets", Vec::new(), table(&head, rows))]
}

/// One entry's row: its name and label, kind, scope and bindings, the current fingerprint and last four characters,
/// its version, when it was last used, its uses, and the row's actions as icon buttons.
fn entry_row<'a>(entry: &'a VaultEntry, state: &'a State) -> Element<'a, Msg> {
    let p = theme::palette();
    let busy = state.busy;
    let now = now_ms();
    let mut name_parts: Vec<Element<'a, Msg>> =
        vec![ui::mono(entry.name.clone(), 12.0, p.ink).into()];
    if !entry.label.is_empty() {
        name_parts.push(ui::faint(entry.label.clone()).into());
    }
    let scope: Element<'a, Msg> = match &entry.scope {
        VaultScope::Plaza => ui::badge("plaza", ui::Tone::Signal),
        VaultScope::Nodes { nodes } => ui::chip_row(
            nodes
                .iter()
                .map(|id| ui::badge(&node_name(&state.nodes, id), ui::Tone::Info))
                .collect(),
        ),
    };
    let bindings: Element<'a, Msg> = if entry.bindings.is_empty() {
        ui::faint("none").into()
    } else {
        ui::chip_row(
            entry
                .bindings
                .iter()
                .map(|b| ui::badge(&binding_name(b, &state.consumers), ui::Tone::Muted))
                .collect(),
        )
    };
    let current = current_version(entry);
    let fingerprint: String = current
        .map(|v| v.fp.chars().take(14).collect())
        .unwrap_or_default();
    let last_four = current.map(|v| v.hint.clone()).unwrap_or_default();
    let rollback = previous_version(entry).and_then(|v| {
        gate(
            busy,
            Msg::Ask(Pending::Rollback {
                id: entry.id.clone(),
                name: entry.name.clone(),
                v,
            }),
        )
    });
    let actions = row![
        ui::btn_sm(
            "",
            Some("refresh"),
            ui::Kind::Ghost,
            gate(busy, Msg::OpenRotate(entry.id.clone(), entry.name.clone()))
        ),
        ui::btn_sm("", Some("clock"), ui::Kind::Ghost, rollback),
        ui::btn_sm(
            "",
            Some("edit"),
            ui::Kind::Ghost,
            gate(busy, Msg::OpenEdit(entry.id.clone()))
        ),
        ui::btn_sm(
            "",
            Some("play"),
            ui::Kind::Ghost,
            gate(busy, Msg::OpenTest(entry.id.clone(), entry.name.clone()))
        ),
        ui::btn_sm(
            "",
            Some("trash"),
            ui::Kind::Danger,
            gate(
                busy,
                Msg::Ask(Pending::RemoveEntry {
                    id: entry.id.clone(),
                    name: entry.name.clone()
                })
            ),
        ),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    cells(vec![
        (3, column(name_parts).spacing(2).into()),
        (2, ui::mono(entry.kind.clone(), 12.0, p.ink_2).into()),
        (2, scope),
        (3, bindings),
        (2, ui::mono(fingerprint, 12.0, p.ink).into()),
        (1, ui::mono(last_four, 12.0, p.ink).into()),
        (
            1,
            ui::mono(format!("v{}", entry.current), 12.0, p.ink).into(),
        ),
        (2, ui::faint(ago(entry.last_used_ms, now)).into()),
        (1, ui::sans(entry.use_count.to_string(), 13.0, p.ink).into()),
        (4, actions.into()),
    ])
}

/// The open dialog, if any. One at a time, the topmost first: a pending confirmation, the recovery code, the
/// unseal, rekey, create, rotate and edit forms, the import, the passphrase and the test.
fn dialog_layer<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    if let Some(pending) = &state.pending {
        return Some(forms::pending_layer(state, pending));
    }
    if let Some(code) = &state.recovery {
        return Some(forms::recovery_layer(state, code));
    }
    if state.unseal_open {
        return Some(forms::unseal_layer(state));
    }
    if let Some(fields) = &state.rekey {
        return Some(forms::rekey_layer(state, fields));
    }
    if let Some(form) = &state.create {
        return Some(forms::create_layer(state, form));
    }
    if let Some(rotate) = &state.rotate {
        return Some(forms::rotate_layer(state, rotate));
    }
    if let Some(edit) = &state.edit {
        return Some(forms::edit_layer(state, edit));
    }
    if state.import_open {
        return Some(audit::import_layer(state));
    }
    if state.pass_open {
        return Some(forms::pass_layer(state));
    }
    if let Some((id, name)) = &state.test {
        return Some(forms::test_layer(state, id, name));
    }
    None
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    let locked = !matches!(&state.status, Some(status) if status.state == "unsealed");
    match &state.status {
        None => match &state.error {
            Some(error) => parts.push(ui::error_line(&format!(
                "The vault could not load. {error}"
            ))),
            None => parts.push(ui::loading()),
        },
        Some(status) => {
            if status.env_file_keys > 0 {
                parts.push(env_banner());
            }
            parts.push(status_card(state, status));
            parts.push(toolbar(state, locked));
            parts.push(ui::tabs(&TABS, state.tab, Msg::Tab));
            parts.extend(tab_content(state, locked));
        }
    }
    if dialog_layer(state).is_none()
        && let Some(failure) = &state.failure
    {
        parts.insert(0, ui::error_line(failure));
    }
    if let Some(notice) = &state.notice {
        parts.insert(0, ui::faint(notice.clone()).into());
    }
    let body: Element<'a, Msg> = column(parts).spacing(16).width(Length::Fill).into();
    let page = ui::scroll(ui::sheet(ui::view_head("Vault", Vec::new()), body)).height(Length::Fill);
    match dialog_layer(state) {
        Some(layer) => Stack::with_children(vec![page.into(), layer]).into(),
        None => page.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_status_parses_a_null_audit_chain() {
        // A fresh or restarted hub sends `audit_ok: null` until its first unlock or audit check.
        let fresh: VaultStatus = serde_json::from_str(
            r#"{"state":"unsealed","slots":[{"id":"s1","kind":"passphrase","created_ms":1.0,"unsealed_with":true},{"id":"s2","kind":"recovery","created_ms":2.0,"unsealed_with":false}],"entries":2,"audit_ok":null,"rekeyed_ms":null,"tampered":null,"env_file_keys":3,"nodes":[]}"#,
        )
        .expect("a status with a null audit chain parses");
        assert_eq!(fresh.audit_ok, None);
        assert_eq!(
            audit_line(fresh.audit_ok),
            "Audit chain not checked until unsealed"
        );
        assert!(fresh.rekeyed_ms.is_none());
        assert_eq!(fresh.env_file_keys, 3);
        assert!(fresh.slots[0].unsealed_with && !fresh.slots[1].unsealed_with);

        let intact: VaultStatus =
            serde_json::from_str(r#"{"state":"unsealed","audit_ok":true,"rekeyed_ms":5.0}"#)
                .unwrap();
        assert_eq!(audit_line(intact.audit_ok), "Audit chain intact");
        assert_eq!(intact.rekeyed_ms, Some(5.0));
        let broken: VaultStatus =
            serde_json::from_str(r#"{"state":"unsealed","audit_ok":false}"#).unwrap();
        assert_eq!(
            audit_line(broken.audit_ok),
            "Audit chain broken, see the Audit tab"
        );
    }

    #[test]
    fn vault_rekey_asks_for_no_secret_of_the_unlock_slot_or_the_key_file() {
        let status: VaultStatus = serde_json::from_str(
            r#"{"state":"unsealed","slots":[
                {"id":"pass","kind":"passphrase","created_ms":1.0,"unsealed_with":true},
                {"id":"rec","kind":"recovery","created_ms":2.0,"unsealed_with":false},
                {"id":"key","kind":"keyfile","created_ms":3.0,"unsealed_with":false}]}"#,
        )
        .unwrap();
        let fields = rekey_fields(&status);
        assert_eq!(
            fields.iter().map(|f| f.slot.as_str()).collect::<Vec<_>>(),
            vec!["rec"]
        );
    }

    #[test]
    fn vault_wipe_drops_every_secret_form() {
        let mut state = State {
            create: Some(SecretForm {
                value: "v".into(),
                ..Default::default()
            }),
            rotate: Some(Rotate {
                id: "a".into(),
                name: "A".into(),
                value: "v".into(),
            }),
            recovery: Some("code".into()),
            add_pass: PassForm {
                pass: "p".into(),
                again: "p".into(),
            },
            ..Default::default()
        };
        state.wipe();
        assert!(state.create.is_none() && state.rotate.is_none() && state.recovery.is_none());
        assert!(state.add_pass.pass.is_empty() && state.add_pass.again.is_empty());
    }
}
