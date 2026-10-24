//! The vault's document: what is stored (the `vault` tables of `plaza.db`) and what the API shows. Only
//! ciphertext, wrapped keys and public metadata are in it. A view never carries a value, and no
//! type here holds plaintext, so none of them implements `Debug`.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::crypto;

/// The whole vault as stored: the `vault` row, with its slots, entries and tombstones as rows of their own tables.
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
#[table(name = "vault")]
pub struct Document {
    pub format: u32,
    pub vault_id: String,
    pub created_ms: u64,
    #[table(child = "vault_slot")]
    pub slots: Vec<Slot>,
    #[table(child = "vault_entry")]
    pub entries: Vec<Entry>,
    #[table(child = "vault_tombstone")]
    pub tombstones: Vec<Tombstone>,
    /// HMAC(K_index, canonical JSON of `index_view`); checked on every unseal.
    #[serde(default)]
    pub index_mac: String,
}

/// One way to unseal: a key file, a passphrase or the recovery code. Each wraps the root key.
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
pub struct Slot {
    pub id: String,
    /// `keyfile`, `passphrase` or `recovery`.
    pub kind: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[table(flatten)]
    pub kdf: Option<Kdf>,
    #[table(flatten)]
    pub wrap: Wrap,
}

/// How a passphrase becomes a key-encryption key. Stored so the cost can be raised later.
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
pub struct Kdf {
    pub alg: String,
    pub salt: String,
    pub memory_kib: u32,
    pub passes: u32,
    pub lanes: u32,
}

/// A ciphertext: a 24-byte nonce and the sealed bytes, both base64.
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
pub struct Wrap {
    pub nonce: String,
    pub ct: String,
}

/// A secret: its scope, its bindings and up to three versions (the current one and two before).
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub label: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub created_by: String,
    /// One row of `vault_entry_scope`, with the node ids of a nodes scope in `vault_scope_node`.
    #[table(child = "vault_entry_scope")]
    pub scope: Scope,
    /// Sorted and without duplicates: the order is part of the AAD, and the rows keep it.
    #[table(list = "vault_entry_binding")]
    pub bindings: Vec<String>,
    pub current: u32,
    #[table(child = "vault_version")]
    pub versions: Vec<Version>,
    /// Usage counters change on every use, so they sit outside the index MAC.
    #[serde(default)]
    pub last_used_ms: u64,
    #[serde(default)]
    pub use_count: u64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Scope {
    Plaza,
    Nodes { nodes: Vec<String> },
}

impl Default for Scope {
    /// What a load gives an entry whose scope row is missing: a nodes scope that names no node, so nothing resolves
    /// through it. The index MAC covers the scope, so a document with such a gap is refused at unseal.
    fn default() -> Self {
        Scope::Nodes { nodes: Vec::new() }
    }
}

impl apz_store::Table for Scope {
    const NAME: &'static str = "vault_scope";

    // The derive cannot express this enum (a variant holds a list, and variants hold columns only), so its table is
    // written by hand: one row with the kind, and the node ids as rows of a list table below it.
    fn define(ctx: apz_store::table::Ctx) -> apz_store::TableDef {
        use apz_store::table as t;
        let kind = t::ColumnDef {
            name: "kind".into(),
            ty: t::SqlType::Text,
            nullable: false,
            check: Some(format!("{} IN ('plaza', 'nodes')", t::quote("kind"))),
        };
        let mut def = t::struct_table(&ctx, Vec::new(), vec![kind]);
        let nodes = t::list_def::<String>(&def, "nodes", Some("vault_scope_node"));
        def.children.push(nodes);
        def
    }

    fn put(
        &self,
        def: &apz_store::TableDef,
        key: Vec<apz_store::table::Value>,
        out: &mut apz_store::RowSet,
    ) -> Result<(), apz_store::StoreError> {
        use apz_store::table as t;
        let (kind, nodes): (&str, &[String]) = match self {
            Scope::Plaza => ("plaza", &[]),
            Scope::Nodes { nodes } => ("nodes", nodes),
        };
        let (key, lead) = t::layout(def, key, Vec::new());
        let full = t::put_row(def, key, lead, vec![t::Value::Text(kind.to_string())], out);
        t::put_list(&def.children[0], &full, nodes.iter(), out)
    }

    fn take(
        def: &apz_store::TableDef,
        row: &apz_store::Row,
        loaded: &apz_store::Loaded,
    ) -> Result<Self, apz_store::StoreError> {
        use apz_store::table as t;
        let (_, mut columns) = t::split(def, row, 0);
        let kind: String = t::take_col(&mut columns, "kind")?;
        match kind.as_str() {
            "plaza" => Ok(Scope::Plaza),
            "nodes" => Ok(Scope::Nodes {
                nodes: t::take_list(&def.children[0], &row.key, loaded)?,
            }),
            other => Err(apz_store::StoreError::invalid(format!(
                "a scope of kind {other}"
            ))),
        }
    }

    fn flat_columns(_prefix: &str, _out: &mut Vec<apz_store::ColumnDef>) {}

    fn flat_put(
        &self,
        _out: &mut Vec<apz_store::table::Value>,
    ) -> Result<(), apz_store::StoreError> {
        Err(apz_store::StoreError::invalid(
            "a scope holds a node list and cannot be flattened",
        ))
    }

    fn flat_take(
        _values: &mut std::vec::IntoIter<apz_store::table::Value>,
    ) -> Result<Self, apz_store::StoreError> {
        Err(apz_store::StoreError::invalid(
            "a scope holds a node list and cannot be flattened",
        ))
    }
}

/// One version of a value. `nonce`/`ct` seal the value under the entry's DEK; `dek_*` wrap the
/// DEK under K_wrap.
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
pub struct Version {
    pub v: u32,
    pub created_ms: u64,
    pub nonce: String,
    pub ct: String,
    pub dek_nonce: String,
    pub dek_ct: String,
    pub fp: String,
    /// The hint (the last 4 characters; values of 20 or more only), sealed under K_index: a 24-byte
    /// nonce and the ciphertext, both base64. Absent for a shorter value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint_nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint_ct: Option<String>,
    /// A plaintext hint, as a file written before hints were encrypted kept it (the key stays `hint`).
    /// Read on an unsealed start and moved into `hint_ct`; nothing writes it again.
    #[serde(rename = "hint", default, skip_serializing_if = "Option::is_none")]
    pub legacy_hint: Option<String>,
}

/// A deleted entry, so that an env-file import never brings its name back.
#[derive(Serialize, Deserialize, Clone, apz_store::Table)]
pub struct Tombstone {
    pub id: String,
    pub name: String,
    pub at_ms: u64,
}

pub fn enc(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn dec(text: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(text)
        .map_err(|_| "a stored value is not base64".to_string())
}

impl Scope {
    /// The scope as canonical JSON, for AAD.
    pub fn canonical(&self) -> String {
        crypto::canonical(&serde_json::to_value(self).unwrap_or(Value::Null))
    }
}

/// The canonical JSON of `bindings`, for AAD.
pub fn bindings_canonical(bindings: &[String]) -> String {
    crypto::canonical(&json!(bindings))
}

/// What the index MAC covers: the slots without their wrapped keys, the entries without their
/// usage counters, and the tombstones. The vault id is bound too.
pub fn index_view(doc: &Document) -> Value {
    let slots: Vec<Value> = doc
        .slots
        .iter()
        .map(|slot| {
            let mut value = serde_json::to_value(slot).unwrap_or(Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.remove("wrap");
            }
            value
        })
        .collect();
    let entries: Vec<Value> = doc
        .entries
        .iter()
        .map(|entry| {
            let mut value = serde_json::to_value(entry).unwrap_or(Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.remove("last_used_ms");
                map.remove("use_count");
            }
            value
        })
        .collect();
    json!({
        "vault_id": doc.vault_id,
        "slots": slots,
        "entries": entries,
        "tombstones": doc.tombstones,
    })
}

/// The entry as the API shows it: no ciphertext, no wrapped keys, no value. Only the fingerprint
/// and the hint of each version. `hint_of` gives a version's hint in clear (the vault decrypts it
/// while unsealed); it is `None` when there is none.
pub fn entry_view(entry: &Entry, hint_of: impl Fn(&Version) -> Option<String>) -> Value {
    let versions: Vec<Value> = entry
        .versions
        .iter()
        .map(|v| json!({ "v": v.v, "created_ms": v.created_ms, "fp": v.fp, "hint": hint_of(v) }))
        .collect();
    json!({
        "id": entry.id,
        "name": entry.name,
        "kind": entry.kind,
        "label": entry.label,
        "created_ms": entry.created_ms,
        "updated_ms": entry.updated_ms,
        "created_by": entry.created_by,
        "scope": entry.scope,
        "bindings": entry.bindings,
        "current": entry.current,
        "versions": versions,
        "last_used_ms": entry.last_used_ms,
        "use_count": entry.use_count,
    })
}

/// A slot as the API shows it.
pub fn slot_view(slot: &Slot) -> Value {
    json!({ "id": slot.id, "kind": slot.kind, "created_ms": slot.created_ms })
}
