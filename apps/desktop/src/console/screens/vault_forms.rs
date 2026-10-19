//! The Vault screen's forms: the bodies it posts to the hub, and the input state behind them.
//! Each builder takes the typed secret out of its state before it checks anything, so a value
//! leaves the screen's state on submit whether or not the request is valid. Nothing here logs one.

use std::fmt;

use iced::widget::{checkbox, column, container, pick_list, row, text_input};
use iced::{Alignment, Element, Length};
use serde_json::{Map, Value, json};

use super::{
    ConsumerRef, Msg, Pending, State, THIS_HUB, failure_line, field, gate, layer, node_name,
    slot_kind_label,
};
use crate::console::ui;
use crate::theme;

/// The hint under the key-file button: the hub writes the file on its own disk, and a file that can be read unseals.
pub const KEYFILE_NOTE: &str = "A key file is written on this hub's disk. Anyone who can read it can unseal the vault without a passphrase.";

/// The hint under the name field, as on the web console.
pub const NAME_HINT: &str =
    "Use 2 to 64 characters: a capital letter, then capitals, digits or underscores.";

/// How the vault is unsealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnsealMethod {
    #[default]
    Passphrase,
    Recovery,
    KeyFile,
}

impl UnsealMethod {
    pub fn label(self) -> &'static str {
        match self {
            UnsealMethod::Passphrase => "Passphrase",
            UnsealMethod::Recovery => "Recovery code",
            UnsealMethod::KeyFile => "Key file",
        }
    }
}

impl fmt::Display for UnsealMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// The unseal form: a method and the one secret it needs (none for the key file).
#[derive(Debug, Clone, Default)]
pub struct UnsealForm {
    pub method: UnsealMethod,
    pub secret: String,
}

/// The body of `POST /v1/vault/unseal`: a passphrase or a recovery code (never both), or `{}` for the key file.
pub fn unseal_body(method: UnsealMethod, secret: &mut String) -> Result<Value, String> {
    let typed = std::mem::take(secret);
    match method {
        UnsealMethod::KeyFile => Ok(json!({})),
        UnsealMethod::Passphrase if typed.is_empty() => Err("Enter the passphrase.".into()),
        UnsealMethod::Passphrase => Ok(json!({ "passphrase": typed })),
        UnsealMethod::Recovery if typed.trim().is_empty() => Err("Enter the recovery code.".into()),
        UnsealMethod::Recovery => Ok(json!({ "recovery_code": typed.trim() })),
    }
}

/// A new passphrase for a passphrase slot, typed twice.
#[derive(Debug, Clone, Default)]
pub struct PassForm {
    pub pass: String,
    pub again: String,
}

/// The body of `POST /v1/vault/slots` for a passphrase. Both fields are cleared on every outcome.
pub fn add_passphrase_body(form: &mut PassForm) -> Result<Value, String> {
    let pass = std::mem::take(&mut form.pass);
    let again = std::mem::take(&mut form.again);
    if pass.is_empty() {
        return Err("Enter a passphrase.".into());
    }
    if pass != again {
        return Err("The two passphrases do not match.".into());
    }
    Ok(json!({ "kind": "passphrase", "passphrase": pass }))
}

/// One passphrase or recovery slot that a rekey must be given the secret for.
#[derive(Debug, Clone, Default)]
pub struct RekeyField {
    pub slot: String,
    pub kind: String,
    pub secret: String,
}

/// The body of `POST /v1/vault/rekey`: each slot's secret, keyed by slot id. Every secret is cleared;
/// when one is missing, nothing is sent.
pub fn rekey_body(fields: &mut [RekeyField]) -> Result<Value, String> {
    let missing = fields.iter().any(|f| f.secret.trim().is_empty());
    let mut secrets = Map::new();
    for f in fields.iter_mut() {
        let secret = std::mem::take(&mut f.secret);
        if !missing {
            secrets.insert(f.slot.clone(), Value::String(secret));
        }
    }
    if missing {
        Err("Give every passphrase and recovery code.".into())
    } else {
        Ok(json!({ "secrets": secrets }))
    }
}

/// The kinds a secret can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecretKind {
    #[default]
    ApiKey,
    Token,
    Password,
    Generic,
}

impl SecretKind {
    pub const ALL: [SecretKind; 4] = [
        SecretKind::ApiKey,
        SecretKind::Token,
        SecretKind::Password,
        SecretKind::Generic,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            SecretKind::ApiKey => "api_key",
            SecretKind::Token => "token",
            SecretKind::Password => "password",
            SecretKind::Generic => "generic",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SecretKind::ApiKey => "API key",
            SecretKind::Token => "Token",
            SecretKind::Password => "Password",
            SecretKind::Generic => "Generic",
        }
    }
}

impl fmt::Display for SecretKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Plaza (every node), or the nodes ticked in `SecretForm::nodes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeMode {
    #[default]
    Plaza,
    Nodes,
}

impl fmt::Display for ScopeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ScopeMode::Plaza => "Plaza (every node)",
            ScopeMode::Nodes => "Specific nodes",
        })
    }
}

/// The create form. `nodes` and `bindings` are (id, ticked) pairs; `value` is cleared on submit.
#[derive(Debug, Clone, Default)]
pub struct SecretForm {
    pub name: String,
    pub kind: SecretKind,
    pub label: String,
    pub value: String,
    pub scope: ScopeMode,
    pub nodes: Vec<(String, bool)>,
    pub bindings: Vec<(String, bool)>,
}

/// The name rule: 2 to 64 characters, a capital letter first, then capitals, digits or underscores.
pub fn valid_name(name: &str) -> bool {
    let len = name.chars().count();
    (2..=64).contains(&len)
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The scope object: `{kind: "plaza"}`, or the ticked nodes.
fn scope_json(mode: ScopeMode, nodes: &[(String, bool)]) -> Result<Value, String> {
    match mode {
        ScopeMode::Plaza => Ok(json!({ "kind": "plaza" })),
        ScopeMode::Nodes => {
            let picked: Vec<&str> = nodes
                .iter()
                .filter(|(_, on)| *on)
                .map(|(id, _)| id.as_str())
                .collect();
            if picked.is_empty() {
                Err("Pick at least one node, or choose Plaza.".into())
            } else {
                Ok(json!({ "kind": "nodes", "nodes": picked }))
            }
        }
    }
}

/// The body of `POST /v1/vault/secrets`. The value is taken out of the form first, so it is cleared
/// on every outcome, including a refused name or scope.
pub fn create_body(form: &mut SecretForm) -> Result<Value, String> {
    let value = std::mem::take(&mut form.value);
    let name = form.name.trim().to_ascii_uppercase();
    if !valid_name(&name) {
        return Err(NAME_HINT.into());
    }
    if value.is_empty() {
        return Err("Enter the value.".into());
    }
    let scope = scope_json(form.scope, &form.nodes)?;
    let bindings: Vec<&str> = form
        .bindings
        .iter()
        .filter(|(_, on)| *on)
        .map(|(b, _)| b.as_str())
        .collect();
    Ok(json!({
        "name": name,
        "kind": form.kind.wire(),
        "label": form.label.trim(),
        "value": value,
        "scope": scope,
        "bindings": bindings,
    }))
}

/// The rotate form: the entry being rotated and the new value, typed once.
#[derive(Debug, Clone, Default)]
pub struct Rotate {
    pub id: String,
    pub name: String,
    pub value: String,
}

/// The body of `PUT /v1/vault/secrets/{id}/value`, the one write path for a new version. The value is cleared.
pub fn rotate_body(value: &mut String) -> Result<Value, String> {
    let typed = std::mem::take(value);
    if typed.is_empty() {
        Err("Enter the new value.".into())
    } else {
        Ok(json!({ "value": typed }))
    }
}

/// The edit form: one entry's label, scope and bindings, as the web's edit dialog sets them. No value.
#[derive(Debug, Clone, Default)]
pub struct EditForm {
    pub id: String,
    pub name: String,
    pub label: String,
    pub scope: ScopeMode,
    /// (node id, ticked), for the nodes the scope can name.
    pub nodes: Vec<(String, bool)>,
    /// (binding, ticked), the consumers plus any binding the entry has that no consumer lists.
    pub bindings: Vec<(String, bool)>,
}

/// The body of `PATCH /v1/vault/secrets/{id}`: the label, the scope and the bindings the form sets.
/// The same rules as the web's: a specific scope needs at least one node.
pub fn edit_body(form: &EditForm) -> Result<Value, String> {
    let scope = scope_json(form.scope, &form.nodes)?;
    let bindings: Vec<&str> = form
        .bindings
        .iter()
        .filter(|(_, on)| *on)
        .map(|(b, _)| b.as_str())
        .collect();
    Ok(json!({ "label": form.label.trim(), "scope": scope, "bindings": bindings }))
}

/// A binding's label in a pick list: the consumer's label and what it wants, when it is known; else the name.
pub fn binding_label(binding: &str, consumers: &[ConsumerRef]) -> String {
    match consumers.iter().find(|c| c.binding == binding) {
        Some(c) if !c.wants.is_empty() => format!("{}  ·  {}", c.label, c.wants),
        Some(c) => c.label.clone(),
        None => binding.to_string(),
    }
}

/// The body of `POST /v1/vault/secrets/{id}/rollback`.
pub fn rollback_body(v: u64) -> Value {
    json!({ "v": v })
}

/// The body of `POST /v1/vault/secrets/{id}/test`: the node to test on, or none for the hub.
pub fn test_body(node: Option<&str>) -> Value {
    match node {
        Some(n) => json!({ "node": n }),
        None => json!({}),
    }
}

/// `/v1/vault/slots/{id}`.
pub fn slot_path(id: &str) -> String {
    format!("/v1/vault/slots/{}", encode(id))
}

/// `/v1/vault/secrets/{id}` with a suffix such as `/rollback`.
pub fn secret_path(id: &str, rest: &str) -> String {
    format!("/v1/vault/secrets/{}{rest}", encode(id))
}

/// Percent-encodes one path segment (everything but the unreserved characters).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The rekey dialog's lede: what a rekey rewrites, and who gives a secret.
const REKEY_LEDE: &str = "Rekey re-encrypts the vault under a new root key. Secret values do not change. Every passphrase and recovery slot is rewritten, so each one must be given now, except the one that unlocked this session.";

/// A row of the vault's node choices, as the test dialog's node field shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeChoice {
    pub id: String,
    pub label: String,
}

impl fmt::Display for NodeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// Whether a binding is one that wants the name (the create form lists those first, as the web does).
fn binding_wants(consumers: &[ConsumerRef], binding: &str, name: &str) -> bool {
    !name.is_empty()
        && consumers
            .iter()
            .any(|c| c.binding == binding && c.wants == name)
}

/// The confirmation for a pending act: what it does, and the button that runs it. Cancel returns to the
/// form it came from, if there is one.
pub(super) fn pending_layer<'a>(state: &'a State, pending: &Pending) -> Element<'a, Msg> {
    let (title, lines, confirm, danger): (String, Vec<String>, &str, bool) = match pending {
        Pending::Seal => (
            "Seal the vault?".to_string(),
            vec!["Sealing locks every secret until the vault is unsealed again.".to_string()],
            "Seal",
            false,
        ),
        Pending::RemoveSlot { kind, .. } => (
            "Remove key slot".to_string(),
            vec![format!(
                "Remove the {} slot? The vault cannot be unsealed with it afterwards.",
                slot_kind_label(kind).to_lowercase()
            )],
            "Remove",
            true,
        ),
        Pending::Rekey => (
            "Rekey the vault?".to_string(),
            vec![
                "Rekey re-encrypts the vault under a new root key. Secret values do not change. Every passphrase and recovery slot is rewritten.".to_string(),
                "The slot this session unlocked with needs no secret, so each other one must be given now.".to_string(),
            ],
            "Rekey",
            false,
        ),
        Pending::RemoveEntry { name, .. } => (
            format!("Delete {name}?"),
            vec![format!(
                "This removes {name}. The removal is recorded in the audit log and blocks an env import from bringing it back."
            )],
            "Delete",
            true,
        ),
        Pending::Rollback { name, v, .. } => (
            format!("Roll back {name}"),
            vec![format!("Roll {name} back to v{v}? It becomes the current value.")],
            "Roll back",
            false,
        ),
        Pending::AddKeyfile => (
            "Add key file slot".to_string(),
            vec![
                "The hub creates the key file on its own disk. The vault can then be unsealed with that file, without a passphrase.".to_string(),
                KEYFILE_NOTE.to_string(),
            ],
            "Add key file slot",
            false,
        ),
        Pending::Recovery => (
            "Create recovery code".to_string(),
            vec!["This replaces the old recovery code.".to_string()],
            "Create recovery code",
            false,
        ),
    };
    let mut body: Vec<Element<'a, Msg>> = lines
        .into_iter()
        .map(|line| ui::prose(line).into())
        .collect();
    body.extend(failure_line(state));
    let kind = if danger {
        ui::Kind::Danger
    } else {
        ui::Kind::Primary
    };
    layer(
        &title,
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Cancel)),
            ui::btn(confirm, None, kind, gate(state.busy, Msg::RunPending)),
        ],
    )
}

/// The new recovery code, shown once: the code in a box with Copy, the tick that says it is saved, then Close.
pub(super) fn recovery_layer<'a>(state: &'a State, code: &'a str) -> Element<'a, Msg> {
    let code_box: Element<'a, Msg> = if state.copy_failed {
        // The clipboard did not take it: the code sits in a read-only field to select.
        text_input("", code)
            .on_input(|_| Msg::Noop)
            .font(theme::MONO)
            .padding(8)
            .style(theme::input)
            .width(Length::Fill)
            .into()
    } else {
        container(ui::mono(code, 13.0, theme::palette().ink))
            .padding([8, 12])
            .width(Length::Fill)
            .style(theme::boxed)
            .into()
    };
    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::prose("Save this code somewhere safe. It is shown once and cannot be shown again.")
            .into(),
    ];
    if state.copy_failed {
        body.push(ui::faint("Could not copy. Select the code and copy it.").into());
    }
    body.push(
        row![
            container(code_box).width(Length::Fill),
            ui::btn("Copy", None, ui::Kind::Ghost, Some(Msg::CopyRecovery)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    body.push(
        checkbox(state.recovery_saved)
            .label("I saved it")
            .on_toggle(Msg::RecoverySaved)
            .into(),
    );
    layer(
        "Recovery code",
        body,
        vec![ui::btn(
            "Close",
            None,
            ui::Kind::Primary,
            state.recovery_saved.then_some(Msg::DismissRecovery),
        )],
    )
}

/// The unseal dialog: a passphrase or a recovery code (only the one picked is sent), the key file when there is one.
pub(super) fn unseal_layer<'a>(state: &'a State) -> Element<'a, Msg> {
    let method = state.unseal.method;
    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::faint("Enter a passphrase or a recovery code. Only the one you pick is sent.").into(),
        ui::seg(
            &[
                (UnsealMethod::Passphrase, "Use a passphrase"),
                (UnsealMethod::Recovery, "Use a recovery code"),
            ],
            method,
            Msg::PickMethod,
        ),
    ];
    if method != UnsealMethod::KeyFile {
        let label = if method == UnsealMethod::Recovery {
            "Recovery code"
        } else {
            "Passphrase"
        };
        let secret = text_input("", &state.unseal.secret)
            .secure(true)
            .on_input(Msg::UnsealSecret)
            .on_submit(Msg::Unseal)
            .padding(8)
            .style(theme::input);
        body.push(field(label, secret.into(), None));
    }
    body.extend(failure_line(state));
    let has_keyfile = state
        .status
        .as_ref()
        .is_some_and(|s| s.slots.iter().any(|slot| slot.kind == "keyfile"));
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if has_keyfile {
        actions.push(ui::btn(
            "Use the key file",
            None,
            ui::Kind::Ghost,
            gate(state.busy, Msg::UnsealKeyfile),
        ));
    }
    actions.push(ui::btn(
        "Cancel",
        None,
        ui::Kind::Ghost,
        Some(Msg::CloseUnseal),
    ));
    actions.push(ui::btn(
        "Unseal",
        None,
        ui::Kind::Primary,
        gate(state.busy, Msg::Unseal),
    ));
    layer("Unseal the vault", body, actions)
}

/// The add-passphrase dialog: the passphrase typed twice.
pub(super) fn pass_layer<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = vec![
        field(
            "Passphrase",
            text_input("", &state.add_pass.pass)
                .secure(true)
                .on_input(Msg::PassPhrase)
                .padding(8)
                .style(theme::input)
                .into(),
            Some("Another way to unseal the vault."),
        ),
        field(
            "Confirm passphrase",
            text_input("", &state.add_pass.again)
                .secure(true)
                .on_input(Msg::PassAgain)
                .on_submit(Msg::AddPassphrase)
                .padding(8)
                .style(theme::input)
                .into(),
            None,
        ),
    ];
    body.extend(failure_line(state));
    layer(
        "Add passphrase",
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::ClosePass)),
            ui::btn(
                "Add passphrase",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::AddPassphrase),
            ),
        ],
    )
}

/// The rekey dialog: each passphrase or recovery slot's secret. Rekey asks for the confirmation before it runs.
pub(super) fn rekey_layer<'a>(state: &'a State, fields: &'a [RekeyField]) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = vec![ui::prose(REKEY_LEDE).into()];
    if fields.is_empty() {
        body.push(ui::faint("No other passphrase or recovery slot needs a secret.").into());
    }
    for (i, slot) in fields.iter().enumerate() {
        let secret = text_input("", &slot.secret)
            .secure(true)
            .on_input(move |v| Msg::RekeyInput(i, v))
            .padding(8)
            .style(theme::input);
        body.push(field(
            &format!("{} ({})", slot_kind_label(&slot.kind), slot.slot),
            secret.into(),
            None,
        ));
    }
    body.extend(failure_line(state));
    layer(
        "Rekey the vault",
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseRekey)),
            ui::btn(
                "Rekey",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::Ask(Pending::Rekey)),
            ),
        ],
    )
}

/// The add-secret dialog: name, kind, label, the value (secure, cleared on save), scope and bindings.
pub(super) fn create_layer<'a>(state: &'a State, form: &'a SecretForm) -> Element<'a, Msg> {
    let name = text_input("API_KEY", &form.name)
        .on_input(Msg::CName)
        .font(theme::MONO)
        .padding(8)
        .style(theme::input);
    let kind =
        pick_list(SecretKind::ALL.to_vec(), Some(form.kind), Msg::CKind).style(theme::pick_list);
    let label = text_input("", &form.label)
        .on_input(Msg::CLabel)
        .padding(8)
        .style(theme::input);
    let value = text_input("", &form.value)
        .secure(true)
        .on_input(Msg::CValue)
        .on_submit(Msg::CSubmit)
        .padding(8)
        .style(theme::input);
    let mut scope: Vec<Element<'a, Msg>> = vec![
        pick_list(
            vec![ScopeMode::Plaza, ScopeMode::Nodes],
            Some(form.scope),
            Msg::CScope,
        )
        .style(theme::pick_list)
        .into(),
    ];
    if form.scope == ScopeMode::Nodes {
        for (i, (id, on)) in form.nodes.iter().enumerate() {
            scope.push(
                checkbox(*on)
                    .label(node_name(&state.nodes, id))
                    .on_toggle(move |v| Msg::CNode(i, v))
                    .into(),
            );
        }
    }
    let wanted = form.name.trim().to_ascii_uppercase();
    let mut order: Vec<usize> = (0..form.bindings.len()).collect();
    order.sort_by_key(|&i| !binding_wants(&state.consumers, &form.bindings[i].0, &wanted));
    let bindings: Vec<Element<'a, Msg>> = order
        .into_iter()
        .map(|i| {
            let (binding, on) = &form.bindings[i];
            checkbox(*on)
                .label(binding_label(binding, &state.consumers))
                .on_toggle(move |v| Msg::CBinding(i, v))
                .into()
        })
        .collect();
    let bindings_col: Element<'a, Msg> = if bindings.is_empty() {
        ui::faint("Nothing asks for a secret yet. You can bind it later.").into()
    } else {
        column(bindings).spacing(6).into()
    };
    let mut body: Vec<Element<'a, Msg>> = vec![
        field("Name", name.into(), Some(NAME_HINT)),
        field("Kind", kind.into(), None),
        field("Label", label.into(), Some("Optional. Shown in lists.")),
        field(
            "Value",
            value.into(),
            Some("Never shown again. It is cleared on save, so type it again after an error."),
        ),
        field("Scope", column(scope).spacing(6).into(), None),
        field("Bindings", bindings_col, None),
    ];
    body.extend(failure_line(state));
    layer(
        "Add secret",
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseCreate)),
            ui::btn(
                "Save secret",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::CSubmit),
            ),
        ],
    )
}

/// The edit dialog: one entry's label, scope and bindings. The value is not part of it: rotate sets a new one.
pub(super) fn edit_layer<'a>(state: &'a State, form: &'a EditForm) -> Element<'a, Msg> {
    let label = text_input("", &form.label)
        .on_input(Msg::EditLabel)
        .padding(8)
        .style(theme::input);
    let mut scope: Vec<Element<'a, Msg>> = vec![
        pick_list(
            vec![ScopeMode::Plaza, ScopeMode::Nodes],
            Some(form.scope),
            Msg::EditScope,
        )
        .style(theme::pick_list)
        .into(),
    ];
    if form.scope == ScopeMode::Nodes {
        for (i, (id, on)) in form.nodes.iter().enumerate() {
            scope.push(
                checkbox(*on)
                    .label(node_name(&state.nodes, id))
                    .on_toggle(move |v| Msg::EditNode(i, v))
                    .into(),
            );
        }
    }
    let bindings: Vec<Element<'a, Msg>> = form
        .bindings
        .iter()
        .enumerate()
        .map(|(i, (binding, on))| {
            checkbox(*on)
                .label(binding_label(binding, &state.consumers))
                .on_toggle(move |v| Msg::EditBinding(i, v))
                .into()
        })
        .collect();
    let bindings_col: Element<'a, Msg> = if bindings.is_empty() {
        ui::faint("Nothing asks for a secret yet.").into()
    } else {
        column(bindings).spacing(6).into()
    };
    let mut body: Vec<Element<'a, Msg>> = vec![
        field("Label", label.into(), None),
        field("Scope", column(scope).spacing(6).into(), None),
        field("Bindings", bindings_col, None),
    ];
    body.extend(failure_line(state));
    layer(
        &format!("Edit {}", form.name),
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseEdit)),
            ui::btn(
                "Save",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::EditSubmit),
            ),
        ],
    )
}

/// The rotate dialog: a new value for one entry, typed once and cleared on save.
pub(super) fn rotate_layer<'a>(state: &'a State, rotate: &'a Rotate) -> Element<'a, Msg> {
    let value = text_input("", &rotate.value)
        .secure(true)
        .on_input(Msg::RotateValue)
        .on_submit(Msg::RotateSubmit)
        .padding(8)
        .style(theme::input);
    let mut body: Vec<Element<'a, Msg>> = vec![field(
        "New value",
        value.into(),
        Some("Never shown again. Each change is a new version."),
    )];
    body.extend(failure_line(state));
    layer(
        &format!("Rotate {}", rotate.name),
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseRotate)),
            ui::btn(
                "Rotate value",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::RotateSubmit),
            ),
        ],
    )
}

/// The test dialog: the probe runs on the chosen node. Only the hub's text is shown, never a value.
pub(super) fn test_layer<'a>(state: &'a State, id: &'a str, name: &str) -> Element<'a, Msg> {
    let mut options: Vec<NodeChoice> = state
        .status
        .as_ref()
        .map(|s| {
            s.nodes
                .iter()
                .map(|n| NodeChoice {
                    id: n.id.clone(),
                    label: if n.id == THIS_HUB {
                        "This hub".to_string()
                    } else if n.name.is_empty() {
                        n.id.clone()
                    } else {
                        n.name.clone()
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    if options.is_empty() {
        options.push(NodeChoice {
            id: THIS_HUB.to_string(),
            label: "This hub".to_string(),
        });
    }
    let current = options.iter().find(|o| o.id == state.test_node).cloned();
    let node =
        pick_list(options, current, |o: NodeChoice| Msg::TestNode(o.id)).style(theme::pick_list);
    let mut body: Vec<Element<'a, Msg>> = vec![field("Node", node.into(), None)];
    if let Some(result) = &state.test_note {
        body.push(
            row![
                ui::badge(
                    if result.ok { "Passed" } else { "Failed" },
                    if result.ok {
                        ui::Tone::Ok
                    } else {
                        ui::Tone::Danger
                    },
                ),
                ui::faint(result.detail.clone()),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    body.extend(failure_line(state));
    let run = if state.test_note.is_some() {
        "Run test again"
    } else {
        "Run test"
    };
    layer(
        &format!("Test {name}"),
        body,
        vec![
            ui::btn("Close", None, ui::Kind::Ghost, Some(Msg::CloseTest)),
            ui::btn(
                run,
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::RunTest(id.to_string())),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_unseal_bodies_send_only_the_chosen_secret() {
        let mut pass = "hunter2".to_string();
        assert_eq!(
            unseal_body(UnsealMethod::Passphrase, &mut pass).unwrap(),
            json!({ "passphrase": "hunter2" })
        );
        assert!(pass.is_empty());
        let mut code = "  ABCD-EF  ".to_string();
        assert_eq!(
            unseal_body(UnsealMethod::Recovery, &mut code).unwrap(),
            json!({ "recovery_code": "ABCD-EF" })
        );
        let mut none = String::new();
        assert_eq!(
            unseal_body(UnsealMethod::KeyFile, &mut none).unwrap(),
            json!({})
        );
        assert!(unseal_body(UnsealMethod::Passphrase, &mut none).is_err());
    }

    #[test]
    fn vault_passphrase_slot_needs_a_match_and_clears_both_fields() {
        let mut f = PassForm {
            pass: "a-long-one".into(),
            again: "a-long-onE".into(),
        };
        assert!(add_passphrase_body(&mut f).is_err());
        assert!(f.pass.is_empty() && f.again.is_empty());
        f.pass = "same".into();
        f.again = "same".into();
        assert_eq!(
            add_passphrase_body(&mut f).unwrap(),
            json!({ "kind": "passphrase", "passphrase": "same" })
        );
        assert!(f.pass.is_empty() && f.again.is_empty());
    }

    #[test]
    fn vault_rekey_sends_every_slot_or_nothing() {
        let mut fields = vec![
            RekeyField {
                slot: "s1".into(),
                kind: "passphrase".into(),
                secret: "p".into(),
            },
            RekeyField {
                slot: "s2".into(),
                kind: "recovery".into(),
                secret: String::new(),
            },
        ];
        assert!(rekey_body(&mut fields).is_err());
        assert!(fields.iter().all(|f| f.secret.is_empty()));
        fields[0].secret = "p".into();
        fields[1].secret = "r".into();
        assert_eq!(
            rekey_body(&mut fields).unwrap(),
            json!({ "secrets": { "s1": "p", "s2": "r" } })
        );
        assert!(fields.iter().all(|f| f.secret.is_empty()));
    }

    #[test]
    fn vault_create_body_scopes_and_binds_only_what_is_ticked() {
        let mut f = SecretForm {
            name: " github_token ".into(),
            kind: SecretKind::Token,
            label: " GH ".into(),
            value: "tok".into(),
            scope: ScopeMode::Nodes,
            nodes: vec![("n0".into(), true), ("n1".into(), false)],
            bindings: vec![("mcp:github".into(), true), ("tool:x".into(), false)],
        };
        let body = create_body(&mut f).unwrap();
        assert_eq!(
            body,
            json!({
                "name": "GITHUB_TOKEN",
                "kind": "token",
                "label": "GH",
                "value": "tok",
                "scope": { "kind": "nodes", "nodes": ["n0"] },
                "bindings": ["mcp:github"],
            })
        );
        assert!(
            f.value.is_empty(),
            "the value must leave the form on submit"
        );
    }

    #[test]
    fn vault_create_body_clears_the_value_even_when_refused() {
        let mut short = SecretForm {
            name: "X".into(),
            value: "secret-value".into(),
            ..Default::default()
        };
        assert!(create_body(&mut short).is_err());
        assert!(short.value.is_empty());
        let mut unscoped = SecretForm {
            name: "API_KEY".into(),
            value: "v".into(),
            scope: ScopeMode::Nodes,
            nodes: vec![("n0".into(), false)],
            ..Default::default()
        };
        assert!(create_body(&mut unscoped).is_err());
        assert!(unscoped.value.is_empty());
    }

    #[test]
    fn vault_name_rule_and_paths() {
        assert!(valid_name("AB") && valid_name("API_KEY_2"));
        assert!(
            !valid_name("A")
                && !valid_name("api_key")
                && !valid_name("1KEY")
                && !valid_name(&"A".repeat(65))
        );
        assert_eq!(
            secret_path("id/1", "/test"),
            "/v1/vault/secrets/id%2F1/test"
        );
        assert_eq!(slot_path("s-1"), "/v1/vault/slots/s-1");
    }

    #[test]
    fn vault_edit_body_sends_label_scope_and_the_ticked_bindings() {
        let mut form = EditForm {
            id: "id1".into(),
            name: "GITHUB_TOKEN".into(),
            label: " GH ".into(),
            scope: ScopeMode::Nodes,
            nodes: vec![("n0".into(), true), ("n1".into(), false)],
            bindings: vec![("mcp:github".into(), true), ("tool:x".into(), false)],
        };
        assert_eq!(
            edit_body(&form).unwrap(),
            json!({
                "label": "GH",
                "scope": { "kind": "nodes", "nodes": ["n0"] },
                "bindings": ["mcp:github"],
            })
        );
        form.scope = ScopeMode::Plaza;
        assert_eq!(
            edit_body(&form).unwrap()["scope"],
            json!({ "kind": "plaza" })
        );
    }

    #[test]
    fn vault_rotate_rollback_and_test_bodies() {
        let mut v = "new".to_string();
        assert_eq!(rotate_body(&mut v).unwrap(), json!({ "value": "new" }));
        assert!(v.is_empty());
        assert!(rotate_body(&mut v).is_err());
        assert_eq!(test_body(Some("n1")), json!({ "node": "n1" }));
        assert_eq!(test_body(None), json!({}));
        assert_eq!(rollback_body(3), json!({ "v": 3 }));
    }
}
