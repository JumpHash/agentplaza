//! The Vault screen's forms: the bodies it posts to the hub, and the input state behind them.
//! Each builder takes the typed secret out of its state before it checks anything, so a value
//! leaves the screen's state on submit whether or not the request is valid. Nothing here logs one.

use std::fmt;

use iced::widget::{checkbox, pick_list, row, text, text_input};
use iced::{Element, Length};
use serde_json::{Map, Value, json};

use super::{
    ConsumerRef, Msg, Pending, State, ago, gate, node_name, now_ms, slot_kind_label, stack,
};
use crate::console::ui;

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

pub(super) fn sealed_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let method = state.unseal.method;
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::faint("Unseal with a passphrase, a recovery code or the key file. Only the one you pick is sent.").into(),
        row(vec![
            ui::faint("Method").into(),
            pick_list(
                vec![UnsealMethod::Passphrase, UnsealMethod::Recovery, UnsealMethod::KeyFile],
                Some(method),
                Msg::PickMethod,
            )
            .into(),
        ])
        .spacing(12)
        .into(),
    ];
    if method != UnsealMethod::KeyFile {
        let placeholder = if method == UnsealMethod::Recovery {
            "Recovery code"
        } else {
            "Passphrase"
        };
        items.push(
            text_input(placeholder, &state.unseal.secret)
                .secure(true)
                .on_input(Msg::UnsealSecret)
                .on_submit(Msg::Unseal)
                .padding(8)
                .into(),
        );
    }
    items.push(ui::btn(
        "Unseal",
        None,
        ui::Kind::Primary,
        gate(state.busy, Msg::Unseal),
    ));
    ui::card("Sealed", stack(items))
}

/// The confirmation for a pending act: what it does, and the button that runs it.
pub(super) fn pending_card<'a>(pending: &Pending, busy: bool) -> Element<'a, Msg> {
    let (title, body, confirm) = match pending {
        Pending::Seal => (
            "Seal the vault",
            "Sealing locks every secret until the vault is unsealed again.".to_string(),
            "Seal",
        ),
        Pending::RemoveSlot { kind, .. } => (
            "Remove a key slot",
            format!("Remove this {} slot? The hub refuses to remove the last slot.", slot_kind_label(kind).to_lowercase()),
            "Remove slot",
        ),
        Pending::Rekey => (
            "Rekey the vault",
            "Rekey re-encrypts the vault under a new root key. Secret values do not change. Every passphrase and recovery slot is rewritten. The slot this session unlocked with needs no secret, so each other one must be given now.".to_string(),
            "Rekey",
        ),
        Pending::RemoveEntry { name, .. } => (
            "Remove a secret",
            format!("Remove {name}? The hub leaves a tombstone, so an env import never brings the name back."),
            "Remove",
        ),
        Pending::Rollback { name, v, .. } => ("Roll back", format!("Roll {name} back to v{v}?"), "Roll back"),
    };
    let danger = matches!(
        pending,
        Pending::RemoveSlot { .. } | Pending::RemoveEntry { .. }
    );
    let kind = if danger {
        ui::Kind::Danger
    } else {
        ui::Kind::Primary
    };
    ui::card(
        title,
        stack(vec![
            ui::faint(body).into(),
            row(vec![
                ui::btn("Keep", None, ui::Kind::Secondary, Some(Msg::Cancel)),
                ui::btn(confirm, None, kind, gate(busy, Msg::RunPending)),
            ])
            .spacing(12)
            .into(),
        ]),
    )
}

/// The new recovery code, shown once.
pub(super) fn recovery_card<'a>(code: &str) -> Element<'a, Msg> {
    ui::card(
        "Recovery code",
        stack(vec![
            ui::faint("Shown once. This replaces the old recovery code. Keep it somewhere safe; it is not shown again.").into(),
            text(code.to_string()).into(),
            row(vec![
                ui::btn("Copy", None, ui::Kind::Primary, Some(Msg::CopyRecovery)),
                ui::btn("Done", None, ui::Kind::Ghost, Some(Msg::DismissRecovery)),
            ])
            .spacing(12)
            .into(),
        ]),
    )
}

/// The key slots, each removable, then the passphrase form and the recovery and rekey actions.
pub(super) fn slots_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(status) = state.status.as_ref() else {
        return ui::card("Key slots", ui::loading());
    };
    let now = now_ms();
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if status.slots.is_empty() {
        items.push(ui::faint("No key slots.").into());
    }
    for (i, slot) in status.slots.iter().enumerate() {
        if i > 0 {
            items.push(ui::hairline());
        }
        items.push(
            row(vec![
                text(slot_kind_label(&slot.kind))
                    .width(Length::FillPortion(3))
                    .into(),
                ui::faint(format!("added {}", ago(slot.created_ms, now)))
                    .width(Length::FillPortion(2))
                    .into(),
                ui::btn_sm(
                    "Remove",
                    None,
                    ui::Kind::Ghost,
                    gate(
                        state.busy,
                        Msg::Ask(Pending::RemoveSlot {
                            id: slot.id.clone(),
                            kind: slot.kind.clone(),
                        }),
                    ),
                ),
            ])
            .spacing(12)
            .into(),
        );
    }
    items.push(ui::hairline());
    items.push(ui::label("Add a passphrase slot").into());
    items.push(
        text_input("Passphrase", &state.add_pass.pass)
            .secure(true)
            .on_input(Msg::PassPhrase)
            .padding(8)
            .into(),
    );
    items.push(
        text_input("Passphrase again", &state.add_pass.again)
            .secure(true)
            .on_input(Msg::PassAgain)
            .on_submit(Msg::AddPassphrase)
            .padding(8)
            .into(),
    );
    items.push(
        ui::faint("A recovery code replaces the old one. Rekey rewrites every passphrase and recovery slot, except the one this session unlocked with.")
            .into(),
    );
    let has_keyfile = status.slots.iter().any(|slot| slot.kind == "keyfile");
    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::btn(
            "Add passphrase",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::AddPassphrase),
        ),
        ui::btn(
            "Make recovery code",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::MakeRecovery),
        ),
        ui::btn(
            "Rekey",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::OpenRekey),
        ),
    ];
    // One key-file slot at most: the hub refuses a second, so the button is shown only without one.
    if !has_keyfile {
        items.push(ui::faint(KEYFILE_NOTE).into());
        actions.push(ui::btn(
            "Add key file",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::AddKeyfile),
        ));
    }
    items.push(row(actions).spacing(12).into());
    ui::card("Key slots", stack(items))
}

/// The rekey form: each passphrase or recovery slot's secret, then the review that confirms it.
pub(super) fn rekey_card<'a>(fields: &'a [RekeyField], busy: bool) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::faint("Give the passphrase or recovery code of each slot except the one this session unlocked with. Each one is rewritten under the new key.")
            .into(),
    ];
    for (i, field) in fields.iter().enumerate() {
        items.push(ui::label(&slot_kind_label(&field.kind)).into());
        items.push(
            text_input("Passphrase or recovery code", &field.secret)
                .secure(true)
                .on_input(move |v| Msg::RekeyInput(i, v))
                .padding(8)
                .into(),
        );
    }
    items.push(
        row(vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseRekey)),
            ui::btn(
                "Review rekey",
                None,
                ui::Kind::Primary,
                gate(busy, Msg::Ask(Pending::Rekey)),
            ),
        ])
        .spacing(12)
        .into(),
    );
    ui::card("Rekey", stack(items))
}

/// The create form: name, kind, label, the value (secure, cleared on submit), scope and bindings.
pub(super) fn create_card<'a>(state: &'a State, form: &'a SecretForm) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![
        text_input("API_KEY", &form.name).on_input(Msg::CName).padding(8).into(),
        ui::faint(NAME_HINT).into(),
        row(vec![
            ui::label("Kind").into(),
            pick_list(SecretKind::ALL.to_vec(), Some(form.kind), Msg::CKind).into(),
        ])
        .spacing(12)
        .into(),
        text_input("Label (optional)", &form.label).on_input(Msg::CLabel).padding(8).into(),
        text_input("Value", &form.value)
            .secure(true)
            .on_input(Msg::CValue)
            .on_submit(Msg::CSubmit)
            .padding(8)
            .into(),
        ui::faint("The value is never shown again. It is cleared on save, so type it again after an error.").into(),
        row(vec![
            ui::label("Scope").into(),
            pick_list(vec![ScopeMode::Plaza, ScopeMode::Nodes], Some(form.scope), Msg::CScope).into(),
        ])
        .spacing(12)
        .into(),
    ];
    if form.scope == ScopeMode::Nodes {
        for (i, (id, on)) in form.nodes.iter().enumerate() {
            items.push(
                checkbox(*on)
                    .label(node_name(&state.nodes, id))
                    .on_toggle(move |v| Msg::CNode(i, v))
                    .into(),
            );
        }
    }
    items.push(ui::label("Bindings").into());
    if form.bindings.is_empty() {
        items.push(ui::faint("Nothing asks for a secret yet. You can bind it later.").into());
    }
    for (i, (binding, on)) in form.bindings.iter().enumerate() {
        items.push(
            checkbox(*on)
                .label(binding_label(binding, &state.consumers))
                .on_toggle(move |v| Msg::CBinding(i, v))
                .into(),
        );
    }
    items.push(
        row(vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseCreate)),
            ui::btn(
                "Save secret",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::CSubmit),
            ),
        ])
        .spacing(12)
        .into(),
    );
    ui::card("New secret", stack(items))
}

/// The edit form: one entry's label, scope and bindings. The value is not part of it, so rotate sets a new one.
pub(super) fn edit_card<'a>(state: &'a State, form: &'a EditForm) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::faint("The value does not change here. Rotate it to set a new one.").into(),
        text_input("Label (optional)", &form.label)
            .on_input(Msg::EditLabel)
            .padding(8)
            .into(),
        row(vec![
            ui::label("Scope").into(),
            pick_list(
                vec![ScopeMode::Plaza, ScopeMode::Nodes],
                Some(form.scope),
                Msg::EditScope,
            )
            .into(),
        ])
        .spacing(12)
        .into(),
    ];
    if form.scope == ScopeMode::Nodes {
        for (i, (id, on)) in form.nodes.iter().enumerate() {
            items.push(
                checkbox(*on)
                    .label(node_name(&state.nodes, id))
                    .on_toggle(move |v| Msg::EditNode(i, v))
                    .into(),
            );
        }
    }
    items.push(ui::label("Bindings").into());
    if form.bindings.is_empty() {
        items.push(ui::faint("Nothing asks for a secret yet.").into());
    }
    for (i, (binding, on)) in form.bindings.iter().enumerate() {
        items.push(
            checkbox(*on)
                .label(binding_label(binding, &state.consumers))
                .on_toggle(move |v| Msg::EditBinding(i, v))
                .into(),
        );
    }
    items.push(
        row(vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseEdit)),
            ui::btn(
                "Save changes",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::EditSubmit),
            ),
        ])
        .spacing(12)
        .into(),
    );
    ui::card(&format!("Edit {}", form.name), stack(items))
}

/// The rotate form: a new value for one entry, typed once and cleared on submit.
pub(super) fn rotate_card<'a>(rotate: &'a Rotate, busy: bool) -> Element<'a, Msg> {
    ui::card(
        &format!("Rotate {}", rotate.name),
        stack(vec![
            ui::faint("Type the new value. It becomes the next version and is cleared on save.")
                .into(),
            text_input("New value", &rotate.value)
                .secure(true)
                .on_input(Msg::RotateValue)
                .on_submit(Msg::RotateSubmit)
                .padding(8)
                .into(),
            row(vec![
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseRotate)),
                ui::btn(
                    "Rotate value",
                    None,
                    ui::Kind::Primary,
                    gate(busy, Msg::RotateSubmit),
                ),
            ])
            .spacing(12)
            .into(),
        ]),
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
    fn vault_edit_body_refuses_a_specific_scope_with_no_node() {
        let form = EditForm {
            scope: ScopeMode::Nodes,
            nodes: vec![("n0".into(), false)],
            ..Default::default()
        };
        assert_eq!(
            edit_body(&form).unwrap_err(),
            "Pick at least one node, or choose Plaza."
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
