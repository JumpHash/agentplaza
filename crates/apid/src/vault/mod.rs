//! The hub's secret vault (docs/design/vault.md): every secret the hub holds, encrypted at rest,
//! scoped to nodes, bound to the consumers that may read it, and audited.
//!
//! [`Store`] is the vault: its state, its unseal and seal, every change and [`Store::resolve`], the
//! one call other modules use to get a value. [`routes`] is the HTTP surface. [`crypto`] holds the
//! primitives, [`slots`] the keyslots, [`audit`] the MAC chain, [`model`] the document, [`disk`] its
//! files, [`dotenv`] the `.env` reader and [`process`] the process hardening.

pub mod audit;
mod crypto;
mod disk;
pub mod dotenv;
mod migrate;
mod model;
pub mod process;
pub mod routes;
mod slots;
#[cfg(test)]
mod tests;
pub use model::Scope;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde_json::{Map, Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::db::Db;
use crate::nodes::Node;
use audit::{Head, Record};
use crypto::Key;
use model::{Document, Entry, Slot, Tombstone, Version};

/// The hub's own machine.
pub const LOCAL_NODE: &str = crate::fleet::LOCAL;
const FORMAT: u32 = 1;
const MAX_VALUE_BYTES: usize = 16 * 1024;
const KINDS: [&str; 4] = ["api_key", "token", "password", "generic"];
const KEEP_VERSIONS: usize = 3;
const HINT_MIN_CHARS: usize = 20;
const UNLOCK_WINDOW_MS: u64 = 60_000;
const UNLOCK_MAX_FAILURES: usize = 5;
const UNLOCK_LOCKOUT_MS: u64 = 600_000;
const PENDING_LIMIT: usize = 256;
const MINUTE_MS: u64 = 60_000;
const VALUE_LABEL: &str = "apz-vault/v1/value";
const DEK_LABEL: &str = "apz-vault/v1/dek";
const HINT_LABEL: &str = "apz-vault/v1/hint";

/// Why a vault operation did not happen. None of the messages carries a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    /// No vault yet (only before the first start finished).
    Uninitialized,
    /// Sealed: nothing resolves until the owner unseals it.
    Sealed,
    /// The vault file was changed outside the API. Nothing resolves; the reason says what failed.
    Tampered(String),
    /// A wrong passphrase or recovery code.
    BadSecret,
    /// Too many failed unseals: wait `retry_ms`.
    Locked {
        retry_ms: u64,
    },
    Conflict(String),
    NotFound,
    /// The secret exists for this consumer, but not on this node.
    OutOfScope,
    Invalid(String),
    /// The last keyslot cannot be removed.
    LastSlot,
    KeyFile(String),
    Storage(String),
    Corrupt(String),
}

impl VaultError {
    /// The text a response or a log line carries. Never a value.
    pub fn message(&self) -> String {
        match self {
            VaultError::Uninitialized => "vault not initialised".into(),
            VaultError::Sealed => "vault sealed".into(),
            VaultError::Tampered(reason) => format!("vault tampered: {reason}"),
            VaultError::BadSecret => "wrong passphrase or recovery code".into(),
            VaultError::Locked { retry_ms } => format!(
                "too many failed unseals; try again in {} s",
                retry_ms / 1000 + 1
            ),
            VaultError::Conflict(message) | VaultError::Invalid(message) => message.clone(),
            VaultError::NotFound => "no such secret".into(),
            VaultError::OutOfScope => "that secret is not scoped to this node".into(),
            VaultError::LastSlot => "the last keyslot cannot be removed".into(),
            VaultError::KeyFile(message)
            | VaultError::Storage(message)
            | VaultError::Corrupt(message) => message.clone(),
        }
    }
}

/// Who asks for a secret, and on whose behalf.
pub struct Consumer {
    /// The binding: `provider:anthropic`, `mcp:github`, `connector:slack`, `hf`, `tool:…`.
    pub binding: String,
    pub on_behalf: OnBehalf,
}

#[derive(Clone)]
pub enum OnBehalf {
    /// An agent. `node` is the node whose CID range holds `cid`, from [`node_for_cid`]; `None` when
    /// a revoked node's range holds it (then nothing resolves).
    Agent { cid: u64, node: Option<String> },
    /// The owner, testing from the console. `None` means the hub's own machine.
    Owner { node: Option<String> },
    /// The hub's background work: plaza-scope entries only. The first background consumer builds it.
    #[allow(dead_code)]
    Hub,
}

impl Consumer {
    /// An agent's consumer. The node comes from the hub's CID ranges, never from the request.
    pub fn agent(nodes: &[Node], cid: u64, binding: &str) -> Consumer {
        Consumer {
            binding: binding.to_string(),
            on_behalf: OnBehalf::Agent {
                cid,
                node: node_for_cid(nodes, cid),
            },
        }
    }
}

/// The vault name of a credential that a provider, connector or tool names: uppercase, with `-` and
/// `.` as `_` (so `Auth::Secret("openai-key")` reads the entry `OPENAI_KEY`).
pub fn name_for(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '-' | '.' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect()
}

/// The node an agent lives on: the one whose CID range holds `cid`. A CID no joined node holds is
/// the hub's own machine, as in `fleet::node_of`. A CID a revoked node holds is `None`: it fails
/// closed rather than falling back to `n0`.
pub fn node_for_cid(nodes: &[Node], cid: u64) -> Option<String> {
    let holds = |n: &Node| cid >= n.cid_base && cid < n.cid_base + n.cid_count;
    if nodes.iter().any(|n| n.revoked && holds(n)) {
        return None;
    }
    Some(
        nodes
            .iter()
            .find(|n| !n.revoked && holds(n))
            .map_or_else(|| LOCAL_NODE.to_string(), |n| n.id.clone()),
    )
}

/// A secret about to be stored.
pub struct NewSecret {
    pub name: String,
    pub kind: String,
    pub label: String,
    pub value: SecretString,
    pub scope: Scope,
    pub bindings: Vec<String>,
}

/// How to unseal.
pub enum Unlock {
    /// The key file (unattended starts; `apz vault unseal` with no secret).
    Keyfile,
    Passphrase(SecretString),
    Recovery(SecretString),
}

/// What a `.env` import did, by name. Reasons never include a value.
#[derive(Default)]
pub struct ImportReport {
    pub imported: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
}

/// An entry as the migration and the connectors see it: its id, whether it is a plaza entry, its bindings. No value.
pub struct Named {
    pub id: String,
    pub plaza: bool,
    pub bindings: Vec<String>,
}

/// A consumer that other modules registered: its binding, a label, and the secret name it wants.
struct Registration {
    binding: String,
    label: String,
    wants: String,
}

/// The unlocked keys. Dropping them zeroizes them.
struct Keys {
    root: Key,
    wrap: Key,
    index: Key,
    audit: Key,
    /// The key-encryption key of the slot that unlocked this session, so a rekey can re-wrap it.
    unlock: Option<(String, Key)>,
    /// The slot that unlocked this session, whatever its kind (the key file included). Status shows it.
    unlocked_slot: Option<String>,
}

/// The aggregation key of a use line: (entry id, binding, node).
type UseKey = (String, String, String);
/// The tally of a use line: (minute, count, entry name, actor).
type UseTally = (u64, u64, String, String);

/// Everything the lock guards.
struct Inner {
    doc: Option<Document>,
    /// Set when the vault file was refused: nothing resolves and nothing is written.
    tamper: Option<String>,
    keys: Option<Arc<Keys>>,
    head: Head,
    audit_ok: Option<bool>,
    rekeyed_ms: Option<u64>,
    /// Failed unseals in the last minute, and the lockout's end.
    failures: Vec<u64>,
    locked_until: u64,
    /// Events from while sealed (no key to MAC them with yet): written at the next unseal.
    pending: Vec<(u64, Record)>,
    /// Successful uses, aggregated per (entry, binding, node) and minute.
    uses: BTreeMap<UseKey, UseTally>,
    usage_dirty: bool,
    consumers: Vec<Registration>,
    /// The variables the env file still holds (the status's `env_file_keys`).
    env_file_keys: usize,
}

/// The vault. It lives in `App` and is shared by every thread.
pub struct Store {
    state_dir: PathBuf,
    key_file: PathBuf,
    db: Option<Arc<Db>>,
    inner: Mutex<Inner>,
}

impl Store {
    /// The vault's files: `state_dir/vault.json` (when there is no Postgres), `vault-audit.jsonl`,
    /// and the key file. Nothing is unlocked yet: call [`Store::boot`].
    pub fn open(state_dir: &Path, key_file: &Path, db: Option<Arc<Db>>) -> Store {
        let store = Store {
            state_dir: state_dir.to_path_buf(),
            key_file: key_file.to_path_buf(),
            db,
            inner: Mutex::new(Inner {
                doc: None,
                tamper: None,
                keys: None,
                head: Head::genesis(),
                audit_ok: None,
                rekeyed_ms: None,
                failures: Vec::new(),
                locked_until: 0,
                pending: Vec::new(),
                uses: BTreeMap::new(),
                usage_dirty: false,
                consumers: Vec::new(),
                env_file_keys: 0,
            }),
        };
        if store.db.is_none() {
            let loaded = disk::read(&store.doc_path());
            let mut inner = store.lock();
            match loaded {
                Ok(Some(text)) => match serde_json::from_str::<Document>(&text) {
                    Ok(doc) => inner.doc = Some(doc),
                    Err(_) => inner.tamper = Some("vault.json is not a vault document".into()),
                },
                Ok(None) => {}
                Err(err) => inner.tamper = Some(format!("vault.json cannot be read: {err}")),
            }
        }
        store
    }

    fn doc_path(&self) -> PathBuf {
        self.state_dir.join("vault.json")
    }

    fn audit_path(&self) -> PathBuf {
        self.state_dir.join("vault-audit.jsonl")
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start-up. A vault that does not exist is initialised with a key-file slot. A vault with a
    /// key-file slot and a key file is unsealed with it; any other vault starts sealed. A key file
    /// that is refused (group or world readable) is an error: the hub does not start.
    pub fn boot(&self, now: u64) -> Result<(), VaultError> {
        let mut inner = self.lock();
        if inner.tamper.is_some() {
            eprintln!(
                "vault: {}; nothing resolves until the file is put right",
                inner.tamper.clone().unwrap_or_default()
            );
            return Ok(());
        }
        if inner.doc.is_none() && self.db.is_some() {
            // Postgres is configured but holds no vault: a vault.json from before it was (copied once,
            // the file is kept). Refusing would be worse than copying: a fresh vault would orphan it.
            match disk::read(&self.doc_path()) {
                Ok(Some(text)) => match serde_json::from_str::<Document>(&text) {
                    Ok(doc) => {
                        self.save(&doc)?;
                        inner.doc = Some(doc);
                        eprintln!(
                            "vault: copied {} into the database; the file can be deleted",
                            self.doc_path().display()
                        );
                    }
                    Err(_) => {
                        inner.tamper = Some("vault.json is not a vault document".into());
                        return Ok(());
                    }
                },
                Ok(None) => {}
                Err(err) => return Err(VaultError::Storage(format!("vault.json: {err}"))),
            }
        }
        if inner.doc.is_none() {
            return self.initialise(&mut inner, now);
        }
        let has_keyfile = inner
            .doc
            .as_ref()
            .is_some_and(|doc| doc.slots.iter().any(|s| s.kind == "keyfile"));
        if has_keyfile && self.key_file.exists() {
            match self.unseal_in(&mut inner, Unlock::Keyfile, "hub", now) {
                Ok(()) => eprintln!("vault: unsealed with the key file"),
                Err(err @ VaultError::KeyFile(_)) => return Err(err),
                Err(err) => eprintln!("vault: starts sealed: {}", err.message()),
            }
        } else {
            eprintln!(
                "vault: starts sealed; give the passphrase or recovery code in the console, or run `apz vault unseal`"
            );
        }
        Ok(())
    }

    fn initialise(&self, inner: &mut Inner, now: u64) -> Result<(), VaultError> {
        let root = crypto::random_key().map_err(VaultError::Storage)?;
        let vault_id = format!(
            "vlt_{}",
            crypto::base32(&crypto::random_vec(10).map_err(VaultError::Storage)?).to_lowercase()
        );
        let kek = if self.key_file.exists() {
            slots::keyfile_read(&self.key_file)?
        } else {
            slots::keyfile_create(&self.key_file)?
        };
        let slot_id = new_id("slt")?;
        let wrap = slots::wrap_root(&kek, &vault_id, &slot_id, &root)?;
        let mut doc = Document {
            format: FORMAT,
            vault_id: vault_id.clone(),
            created_ms: now,
            slots: vec![Slot {
                id: slot_id,
                kind: "keyfile".into(),
                created_ms: now,
                kdf: None,
                wrap,
            }],
            entries: Vec::new(),
            tombstones: Vec::new(),
            index_mac: String::new(),
        };
        let mut unlocked = derive(&root, &vault_id);
        unlocked.unlocked_slot = Some(doc.slots[0].id.clone());
        let keys = Arc::new(unlocked);
        sign(&keys, &mut doc);
        self.save(&doc)?;
        inner.doc = Some(doc);
        inner.keys = Some(keys.clone());
        inner.head = Head::genesis();
        inner.audit_ok = Some(true);
        self.audit(
            inner,
            &keys.audit,
            Record::new("init", "hub").detail("keyfile slot"),
            now,
        );
        eprintln!(
            "vault: initialised with a key file at {}",
            self.key_file.display()
        );
        Ok(())
    }

    /// Unseals with `unlock`, as `actor`. Failures are rate-limited and audited.
    pub fn unseal(&self, unlock: Unlock, actor: &str, now: u64) -> Result<(), VaultError> {
        let mut inner = self.lock();
        self.unseal_in(&mut inner, unlock, actor, now)
    }

    fn unseal_in(
        &self,
        inner: &mut Inner,
        unlock: Unlock,
        actor: &str,
        now: u64,
    ) -> Result<(), VaultError> {
        if inner.keys.is_some() {
            return Ok(());
        }
        if let Some(reason) = inner.tamper.clone() {
            return Err(VaultError::Tampered(reason));
        }
        let Some(doc) = inner.doc.clone() else {
            return Err(VaultError::Uninitialized);
        };
        if now < inner.locked_until {
            self.pend(
                inner,
                now,
                Record::new("unseal_failed", actor).detail("locked out"),
            );
            return Err(VaultError::Locked {
                retry_ms: inner.locked_until - now,
            });
        }
        let attempt = match &unlock {
            Unlock::Keyfile => {
                let slot = doc
                    .slots
                    .iter()
                    .find(|s| s.kind == "keyfile")
                    .ok_or_else(|| VaultError::Invalid("this vault has no key file slot".into()))?;
                // A refused key file is an error about the file, not a wrong attempt.
                let kek = slots::keyfile_read(&self.key_file)?;
                match slots::unwrap_root(&kek, &doc.vault_id, slot) {
                    Ok(root) => Ok((root, slot.id.clone(), None)),
                    Err(_) => Err(VaultError::BadSecret),
                }
            }
            Unlock::Passphrase(secret) => {
                let candidates: Vec<&Slot> = doc
                    .slots
                    .iter()
                    .filter(|s| s.kind == "passphrase")
                    .collect();
                if candidates.is_empty() {
                    return Err(VaultError::Invalid(
                        "this vault has no passphrase slot".into(),
                    ));
                }
                let mut found = Err(VaultError::BadSecret);
                for slot in candidates {
                    let kdf = slot.kdf.as_ref().ok_or_else(|| {
                        VaultError::Corrupt("a passphrase slot has no KDF".into())
                    })?;
                    let kek = slots::kek_from_passphrase(secret.expose_secret(), kdf)?;
                    if let Ok(root) = slots::unwrap_root(&kek, &doc.vault_id, slot) {
                        found = Ok((root, slot.id.clone(), Some(kek)));
                        break;
                    }
                }
                found
            }
            Unlock::Recovery(secret) => {
                let slot = doc.slots.iter().find(|s| s.kind == "recovery");
                match (slots::parse_recovery_code(secret.expose_secret()), slot) {
                    (Some(code), Some(slot)) => {
                        let kek = slots::kek_from_recovery(&code, &doc.vault_id);
                        match slots::unwrap_root(&kek, &doc.vault_id, slot) {
                            Ok(root) => Ok((root, slot.id.clone(), Some(kek))),
                            Err(_) => Err(VaultError::BadSecret),
                        }
                    }
                    (None, Some(_)) => Err(VaultError::BadSecret),
                    (_, None) => Err(VaultError::Invalid(
                        "this vault has no recovery slot".into(),
                    )),
                }
            }
        };
        match attempt {
            Err(VaultError::BadSecret) => Err(self.failed(inner, now, actor)),
            Err(other) => Err(other),
            Ok((root, slot_id, kek)) => {
                let mut keys = derive(&root, &doc.vault_id);
                keys.unlocked_slot = Some(slot_id.clone());
                if let Some(kek) = kek {
                    keys.unlock = Some((slot_id, kek));
                }
                if !index_ok(&keys, &doc) {
                    let reason =
                        "the index MAC does not match: the vault file was changed outside the API"
                            .to_string();
                    let keys = Arc::new(keys);
                    self.audit(
                        inner,
                        &keys.audit,
                        Record::new("tamper", actor).detail("index MAC"),
                        now,
                    );
                    inner.tamper = Some(reason.clone());
                    return Err(VaultError::Tampered(reason));
                }
                let text = disk::read(&self.audit_path())
                    .map_err(|e| VaultError::Storage(e.to_string()))?
                    .unwrap_or_default();
                let verdict = audit::verify(&text, &keys.audit);
                inner.head = verdict.head.clone();
                inner.audit_ok = Some(verdict.ok);
                inner.rekeyed_ms = verdict.last_rekey_ms;
                if let Some(line) = verdict.broken_at {
                    eprintln!("vault: the audit chain breaks at line {line}");
                }
                let keys = Arc::new(keys);
                inner.keys = Some(keys.clone());
                inner.failures.clear();
                inner.locked_until = 0;
                for (at, record) in std::mem::take(&mut inner.pending) {
                    self.audit(inner, &keys.audit, record, at);
                }
                self.audit(inner, &keys.audit, Record::new("unseal", actor), now);
                self.encrypt_legacy_hints(inner, &keys, now);
                Ok(())
            }
        }
    }

    /// Moves every plaintext hint of an older file into `hint_ct` (sealed under K_index), then signs
    /// and saves the document with one `migrate` line. Once done no version has a plaintext hint, so
    /// later unseals do nothing. A failed save is logged; the hints stay plaintext until the next try.
    fn encrypt_legacy_hints(&self, inner: &mut Inner, keys: &Keys, now: u64) {
        let Some(doc) = inner.doc.as_ref() else {
            return;
        };
        if !doc
            .entries
            .iter()
            .any(|e| e.versions.iter().any(|v| v.legacy_hint.is_some()))
        {
            return;
        }
        let mut doc = doc.clone();
        for entry in doc.entries.iter_mut() {
            for version in entry.versions.iter_mut() {
                let Some(plain) = version.legacy_hint.take() else {
                    continue;
                };
                match seal_hint(keys, &entry.id, version.v, &plain) {
                    Ok((nonce, ct)) => {
                        version.hint_nonce = Some(nonce);
                        version.hint_ct = Some(ct);
                    }
                    // The clone is dropped: the plaintext stays in the file until a later unseal succeeds.
                    Err(err) => {
                        eprintln!("vault: a hint could not be encrypted: {}", err.message());
                        return;
                    }
                }
            }
        }
        let record = Record::new("migrate", "hub").detail("hints encrypted");
        if let Err(err) = self.commit(inner, keys, doc, vec![record], now) {
            eprintln!(
                "vault: the hints could not be saved encrypted: {}",
                err.message()
            );
        }
    }

    /// Counts a failed unseal; the fifth within a minute starts the lockout.
    fn failed(&self, inner: &mut Inner, now: u64, actor: &str) -> VaultError {
        inner
            .failures
            .retain(|t| now.saturating_sub(*t) < UNLOCK_WINDOW_MS);
        inner.failures.push(now);
        self.pend(
            inner,
            now,
            Record::new("unseal_failed", actor).detail("wrong passphrase or recovery code"),
        );
        if inner.failures.len() >= UNLOCK_MAX_FAILURES {
            inner.failures.clear();
            inner.locked_until = now + UNLOCK_LOCKOUT_MS;
            return VaultError::Locked {
                retry_ms: UNLOCK_LOCKOUT_MS,
            };
        }
        VaultError::BadSecret
    }

    /// Holds an event for the next unseal, when there is no key to chain it with.
    fn pend(&self, inner: &mut Inner, now: u64, record: Record) {
        if inner.pending.len() < PENDING_LIMIT {
            inner.pending.push((now, record));
        }
    }

    /// Seals: flushes the aggregated use lines, saves the counters, then forgets the keys.
    pub fn seal(&self, actor: &str, now: u64) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let Some(keys) = inner.keys.clone() else {
            return Ok(());
        };
        self.flush_uses(&mut inner, &keys, now, true);
        if let Some(doc) = &inner.doc
            && let Err(err) = self.save(doc)
        {
            eprintln!(
                "vault: the usage counters could not be saved: {}",
                err.message()
            );
        }
        self.audit(&mut inner, &keys.audit, Record::new("seal", actor), now);
        inner.keys = None;
        inner.usage_dirty = false;
        Ok(())
    }

    /// The saver's tick (every 3 s): writes the aggregated use lines of past minutes and the counters.
    pub fn flush(&self, now: u64) {
        let mut inner = self.lock();
        let Some(keys) = inner.keys.clone() else {
            return;
        };
        self.flush_uses(&mut inner, &keys, now, false);
        if inner.usage_dirty {
            if let Some(doc) = &inner.doc
                && let Err(err) = self.save(doc)
            {
                eprintln!(
                    "vault: the usage counters could not be saved: {}",
                    err.message()
                );
            }
            inner.usage_dirty = false;
        }
    }

    /// The audit chain's last known state: `None` before any check.
    pub fn audit_ok(&self) -> Option<bool> {
        self.lock().audit_ok
    }

    /// Checks the whole audit chain now (needs the keys).
    pub fn check_audit(&self) -> Result<audit::Verdict, VaultError> {
        let inner = self.lock();
        let keys = inner.keys.clone().ok_or_else(|| self.unavailable(&inner))?;
        drop(inner);
        let text = disk::read(&self.audit_path())
            .map_err(|e| VaultError::Storage(e.to_string()))?
            .unwrap_or_default();
        let verdict = audit::verify(&text, &keys.audit);
        self.lock().audit_ok = Some(verdict.ok);
        Ok(verdict)
    }

    /// The audit lines after sequence number `after`, at most `limit`. They carry no values.
    pub fn audit_page(&self, after: u64, limit: usize) -> Result<Vec<Value>, VaultError> {
        let text = disk::read(&self.audit_path())
            .map_err(|e| VaultError::Storage(e.to_string()))?
            .unwrap_or_default();
        Ok(audit::page(&text, after, limit))
    }

    /// Adds a passphrase slot: Argon2id under a fresh salt, at the current cost.
    pub fn add_passphrase(
        &self,
        passphrase: &SecretString,
        actor: &str,
        now: u64,
    ) -> Result<String, VaultError> {
        slots::check_passphrase(passphrase.expose_secret())?;
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let slot_id = new_id("slt")?;
        let kdf = slots::new_kdf()?;
        let kek = slots::kek_from_passphrase(passphrase.expose_secret(), &kdf)?;
        let wrap = slots::wrap_root(&kek, &doc.vault_id, &slot_id, &keys.root)?;
        doc.slots.push(Slot {
            id: slot_id.clone(),
            kind: "passphrase".into(),
            created_ms: now,
            kdf: Some(kdf),
            wrap,
        });
        self.commit(
            &mut inner,
            &keys,
            doc,
            vec![Record::new("slot_added", actor).detail(format!("passphrase {slot_id}"))],
            now,
        )?;
        Ok(slot_id)
    }

    /// Adds the key-file slot: creates the key file (refused when one exists).
    pub fn add_keyfile(&self, actor: &str, now: u64) -> Result<String, VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        if doc.slots.iter().any(|s| s.kind == "keyfile") {
            return Err(VaultError::Conflict(
                "this vault already has a key file slot".into(),
            ));
        }
        let kek = slots::keyfile_create(&self.key_file)?;
        let slot_id = new_id("slt")?;
        let wrap = slots::wrap_root(&kek, &doc.vault_id, &slot_id, &keys.root)?;
        doc.slots.push(Slot {
            id: slot_id.clone(),
            kind: "keyfile".into(),
            created_ms: now,
            kdf: None,
            wrap,
        });
        if let Err(err) = self.commit(
            &mut inner,
            &keys,
            doc,
            vec![Record::new("slot_added", actor).detail(format!("keyfile {slot_id}"))],
            now,
        ) {
            slots::keyfile_shred(&self.key_file);
            return Err(err);
        }
        Ok(slot_id)
    }

    /// Removes a slot. The last slot stays. Removing a key-file slot destroys the key file.
    pub fn remove_slot(&self, id: &str, actor: &str, now: u64) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        if doc.slots.len() <= 1 {
            return Err(VaultError::LastSlot);
        }
        let index = doc
            .slots
            .iter()
            .position(|s| s.id == id)
            .ok_or(VaultError::NotFound)?;
        let removed = doc.slots.remove(index);
        self.commit(
            &mut inner,
            &keys,
            doc,
            vec![Record::new("slot_removed", actor).detail(format!("{} {id}", removed.kind))],
            now,
        )?;
        if removed.kind == "keyfile" {
            slots::keyfile_shred(&self.key_file);
        }
        Ok(())
    }

    /// A new recovery code. It replaces the old recovery slot. The code is returned once.
    pub fn make_recovery(&self, actor: &str, now: u64) -> Result<Zeroizing<String>, VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let (code, text) = slots::new_recovery_code()?;
        let kek = slots::kek_from_recovery(&code, &doc.vault_id);
        doc.slots.retain(|s| s.kind != "recovery");
        let slot_id = new_id("slt")?;
        let wrap = slots::wrap_root(&kek, &doc.vault_id, &slot_id, &keys.root)?;
        doc.slots.push(Slot {
            id: slot_id,
            kind: "recovery".into(),
            created_ms: now,
            kdf: None,
            wrap,
        });
        self.commit(
            &mut inner,
            &keys,
            doc,
            vec![Record::new("recovery_made", actor)],
            now,
        )?;
        Ok(text)
    }

    /// A new root key: every data key and every keyslot is re-wrapped, and the index and audit keys
    /// are new. A passphrase or recovery slot needs its secret, unless it is the one that unlocked
    /// this session; nothing changes unless every slot can be re-wrapped. Values are not touched.
    pub fn rekey(
        &self,
        supplied: &[(String, SecretString)],
        actor: &str,
        now: u64,
    ) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let (old, mut doc) = self.active(&inner)?;
        let new_root = crypto::random_key().map_err(VaultError::Storage)?;
        let vault_id = doc.vault_id.clone();
        let mut new_keys = derive(&new_root, &vault_id);
        for entry in doc.entries.iter_mut() {
            for version in entry.versions.iter_mut() {
                let dek = unwrap_dek(&old, &entry.id, version)?;
                let (nonce, ct) =
                    crypto::seal(&new_keys.wrap, &dek_aad(&entry.id, version.v), &dek[..])
                        .map_err(VaultError::Storage)?;
                version.dek_nonce = model::enc(&nonce);
                version.dek_ct = model::enc(&ct);
                rehome_hint(&old, &new_keys, &entry.id, version);
            }
        }
        let mut new_keyfile: Option<Key> = None;
        for slot in doc.slots.iter_mut() {
            let kek: Key = if slot.kind == "keyfile" {
                let kek = crypto::random_key().map_err(VaultError::Storage)?;
                new_keyfile = Some(kek.clone());
                kek
            } else if let Some(kek) = old
                .unlock
                .as_ref()
                .and_then(|(id, kek)| (*id == slot.id).then(|| kek.clone()))
            {
                kek
            } else {
                let secret = supplied
                    .iter()
                    .find(|(id, _)| *id == slot.id)
                    .map(|(_, s)| s)
                    .ok_or_else(|| {
                        VaultError::Invalid(format!(
                            "slot {} needs its passphrase or recovery code to be re-wrapped",
                            slot.id
                        ))
                    })?;
                let kek = match slot.kind.as_str() {
                    "passphrase" => {
                        let kdf = slot.kdf.as_ref().ok_or_else(|| {
                            VaultError::Corrupt("a passphrase slot has no KDF".into())
                        })?;
                        slots::kek_from_passphrase(secret.expose_secret(), kdf)?
                    }
                    _ => {
                        let code = slots::parse_recovery_code(secret.expose_secret())
                            .ok_or(VaultError::BadSecret)?;
                        slots::kek_from_recovery(&code, &vault_id)
                    }
                };
                // Prove the secret before re-wrapping: a typo must not wrap the root under garbage.
                slots::unwrap_root(&kek, &vault_id, slot).map_err(|_| VaultError::BadSecret)?;
                kek
            };
            slot.wrap = slots::wrap_root(&kek, &vault_id, &slot.id, &new_root)?;
        }
        let previous = new_keyfile
            .as_ref()
            .and_then(|_| slots::keyfile_read(&self.key_file).ok());
        if let Some(kek) = &new_keyfile {
            slots::keyfile_replace(&self.key_file, kek)?;
        }
        new_keys.unlock = old.unlock.clone();
        new_keys.unlocked_slot = old.unlocked_slot.clone();
        // The rekey line starts the new MAC key's chain: it is written with the new audit key.
        if let Err(err) = self.commit(
            &mut inner,
            &new_keys,
            doc,
            vec![Record::new("rekey", actor)],
            now,
        ) {
            if new_keyfile.is_some() {
                match previous {
                    Some(kek) => {
                        let _ = slots::keyfile_replace(&self.key_file, &kek);
                    }
                    None => slots::keyfile_shred(&self.key_file),
                }
            }
            return Err(err);
        }
        inner.keys = Some(Arc::new(new_keys));
        inner.rekeyed_ms = Some(now);
        Ok(())
    }

    /// The vault's entries, as views (no values). A sealed vault answers nothing; an unsealed one shows
    /// each version's hint.
    pub fn list(&self) -> Result<Vec<Value>, VaultError> {
        let inner = self.lock();
        let keys = inner.keys.clone().ok_or_else(|| self.unavailable(&inner))?;
        let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
        Ok(doc
            .entries
            .iter()
            .map(|entry| view_of(&keys, entry))
            .collect())
    }

    /// The entry `id` as a view (the hints are decrypted, as in [`Store::list`]).
    pub fn view(&self, id: &str) -> Result<Value, VaultError> {
        let inner = self.lock();
        let keys = inner.keys.clone().ok_or_else(|| self.unavailable(&inner))?;
        let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
        let index = position(&doc.entries, id)?;
        Ok(view_of(&keys, &doc.entries[index]))
    }

    /// Creates a secret. Returns its id.
    pub fn create(&self, new: NewSecret, actor: &str, now: u64) -> Result<String, VaultError> {
        self.add(new, actor, "create", now)
    }

    /// The hook for imports (the env file, and the migration of the old files): one secret, recorded
    /// as `import`. Returns its id.
    pub fn import_entry(
        &self,
        new: NewSecret,
        actor: &str,
        now: u64,
    ) -> Result<String, VaultError> {
        self.add(new, actor, "import", now)
    }

    fn add(
        &self,
        new: NewSecret,
        actor: &str,
        event: &str,
        now: u64,
    ) -> Result<String, VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let entry = build_entry(&keys, &doc, new, now)?;
        let (id, name) = (entry.id.clone(), entry.name.clone());
        doc.entries.push(entry);
        self.commit(
            &mut inner,
            &keys,
            doc,
            vec![Record::new(event, actor).entry(&id, &name)],
            now,
        )?;
        Ok(id)
    }

    /// A new version of entry `id`. The previous versions stay for rollback (up to two).
    pub fn rotate(
        &self,
        id: &str,
        value: &SecretString,
        actor: &str,
        now: u64,
    ) -> Result<(), VaultError> {
        validate_value(value.expose_secret())?;
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let index = position(&doc.entries, id)?;
        let entry = &mut doc.entries[index];
        let version = entry.versions.iter().map(|v| v.v).max().unwrap_or(0) + 1;
        let sealed = seal_version(
            &keys,
            &entry.id,
            version,
            &entry.name,
            &entry.scope,
            &entry.bindings,
            value.expose_secret(),
            now,
        )?;
        entry.versions.push(sealed);
        prune(entry);
        entry.current = version;
        entry.updated_ms = now;
        let record = Record::new("rotate", actor)
            .entry(&entry.id, &entry.name)
            .detail(format!("version {version}"));
        self.commit(&mut inner, &keys, doc, vec![record], now)
    }

    /// Makes version `v` the current one.
    pub fn rollback(&self, id: &str, v: u32, actor: &str, now: u64) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let index = position(&doc.entries, id)?;
        let entry = &mut doc.entries[index];
        if !entry.versions.iter().any(|x| x.v == v) {
            return Err(VaultError::NotFound);
        }
        entry.current = v;
        entry.updated_ms = now;
        let record = Record::new("rollback", actor)
            .entry(&entry.id, &entry.name)
            .detail(format!("to version {v}"));
        self.commit(&mut inner, &keys, doc, vec![record], now)
    }

    /// Changes the label, the scope or the bindings. A scope or binding change re-encrypts the kept
    /// versions under the new AAD, so rollback keeps working; the values never leave the vault.
    pub fn patch(
        &self,
        id: &str,
        label: Option<String>,
        scope: Option<Scope>,
        bindings: Option<Vec<String>>,
        actor: &str,
        now: u64,
    ) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let index = position(&doc.entries, id)?;
        let (name, old_scope, old_bindings, entry_id) = {
            let e = &doc.entries[index];
            (
                e.name.clone(),
                e.scope.clone(),
                e.bindings.clone(),
                e.id.clone(),
            )
        };
        let new_scope = match scope {
            Some(scope) => validate_scope(scope)?,
            None => old_scope.clone(),
        };
        let new_bindings = match bindings {
            Some(list) => validate_bindings(&list)?,
            None => old_bindings.clone(),
        };
        check_scope(&doc.entries, &name, &new_scope, Some(&entry_id))?;
        let mut records = Vec::new();
        let entry = &mut doc.entries[index];
        if new_scope != old_scope || new_bindings != old_bindings {
            for version in entry.versions.iter_mut() {
                rewrap_value(
                    &keys,
                    &entry_id,
                    &name,
                    (&old_scope, &old_bindings),
                    (&new_scope, &new_bindings),
                    version,
                )?;
            }
            if new_scope != old_scope {
                records.push(
                    Record::new("scope", actor)
                        .entry(&entry_id, &name)
                        .detail(scope_text(&new_scope)),
                );
            }
            if new_bindings != old_bindings {
                records.push(
                    Record::new("bindings", actor)
                        .entry(&entry_id, &name)
                        .detail(new_bindings.join(",")),
                );
            }
            entry.scope = new_scope;
            entry.bindings = new_bindings;
        }
        if let Some(label) = label {
            entry.label = clean_label(&label)?;
        }
        entry.updated_ms = now;
        self.commit(&mut inner, &keys, doc, records, now)
    }

    /// Removes `binding` from every entry that names it (a deleted MCP server takes its `mcp:<id>` with it).
    /// Each entry goes through [`Store::patch`]: its kept versions are re-encrypted under the new AAD, the
    /// index is MACed again, and the audit gets a `bindings` line per entry. An entry left with no bindings
    /// stays, and nothing can resolve it. Returns how many entries changed.
    pub fn remove_binding(
        &self,
        binding: &str,
        actor: &str,
        now: u64,
    ) -> Result<usize, VaultError> {
        let targets: Vec<(String, Vec<String>)> = {
            let inner = self.lock();
            let (_, doc) = self.active(&inner)?;
            doc.entries
                .iter()
                .filter(|e| e.bindings.iter().any(|b| b == binding))
                .map(|e| {
                    (
                        e.id.clone(),
                        e.bindings
                            .iter()
                            .filter(|b| b.as_str() != binding)
                            .cloned()
                            .collect(),
                    )
                })
                .collect()
        };
        for (id, rest) in &targets {
            self.patch(id, None, None, Some(rest.clone()), actor, now)?;
        }
        Ok(targets.len())
    }

    /// Deletes entry `id` and leaves a tombstone, so an env import never brings its name back.
    pub fn delete(&self, id: &str, actor: &str, now: u64) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let index = position(&doc.entries, id)?;
        let mut removed = doc.entries.remove(index);
        for version in removed.versions.iter_mut() {
            wipe(version);
        }
        doc.tombstones.push(Tombstone {
            id: removed.id.clone(),
            name: removed.name.clone(),
            at_ms: now,
        });
        let record = Record::new("delete", actor).entry(&removed.id, &removed.name);
        self.commit(&mut inner, &keys, doc, vec![record], now)
    }

    /// Checks a secret the way its bound consumer would: resolves it for its first binding on `node`
    /// (the hub's machine by default). Nothing leaves the vault. Returns the binding.
    pub fn test(
        &self,
        id: &str,
        node: Option<String>,
        now: u64,
    ) -> Result<(String, String), VaultError> {
        let (name, binding) = {
            let inner = self.lock();
            if inner.keys.is_none() {
                return Err(self.unavailable(&inner));
            }
            let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
            let entry = &doc.entries[position(&doc.entries, id)?];
            let binding =
                entry.bindings.first().cloned().ok_or_else(|| {
                    VaultError::Invalid("the secret has no bindings to test".into())
                })?;
            (entry.name.clone(), binding)
        };
        let node = node.unwrap_or_else(|| LOCAL_NODE.to_string());
        let consumer = Consumer {
            binding: binding.clone(),
            on_behalf: OnBehalf::Owner {
                node: Some(node.clone()),
            },
        };
        let (_, found) = self.resolve_entry(&name, &consumer, now)?;
        if found != id {
            return Err(VaultError::Invalid(
                "another secret of that name serves this binding on that node".into(),
            ));
        }
        Ok((binding, node))
    }

    /// Imports a `.env` file: each `NAME=value` becomes an entry in `scope`, bound as `bindings[NAME]`.
    /// Names that are invalid, empty, duplicated in the file, tombstoned, or already present in that
    /// scope are skipped with a reason. Nothing is logged but names and reasons.
    pub fn import_dotenv(
        &self,
        text: &str,
        scope: Scope,
        bindings: &BTreeMap<String, Vec<String>>,
        actor: &str,
        now: u64,
    ) -> Result<ImportReport, VaultError> {
        let lines = dotenv::parse(text).map_err(VaultError::Invalid)?;
        let scope = validate_scope(scope)?;
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for line in &lines {
            *counts.entry(line.name.as_str()).or_insert(0) += 1;
        }
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let mut report = ImportReport::default();
        let mut records = Vec::new();
        for line in &lines {
            let name = line.name.as_str();
            let skip = |reason: &str, report: &mut ImportReport| {
                report.skipped.push((name.to_string(), reason.to_string()))
            };
            if counts.get(name).copied().unwrap_or(0) > 1 {
                skip("duplicate in the file", &mut report);
                continue;
            }
            if !valid_name(name) {
                skip("invalid name", &mut report);
                continue;
            }
            if line.value.is_empty() {
                skip("empty value", &mut report);
                continue;
            }
            if doc.tombstones.iter().any(|t| t.name == name) {
                skip("deleted before; not imported again", &mut report);
                continue;
            }
            let new = NewSecret {
                name: name.to_string(),
                kind: "generic".into(),
                label: "imported from a .env file".into(),
                value: SecretString::from(line.value.as_str().to_string()),
                scope: scope.clone(),
                bindings: bindings.get(name).cloned().unwrap_or_default(),
            };
            match build_entry(&keys, &doc, new, now) {
                Ok(entry) => {
                    records.push(
                        Record::new("import", actor)
                            .entry(&entry.id, &entry.name)
                            .detail("dotenv"),
                    );
                    report.imported.push((entry.name.clone(), entry.id.clone()));
                    doc.entries.push(entry);
                }
                Err(err) => skip(&err.message(), &mut report),
            }
        }
        if !report.imported.is_empty() {
            self.commit(&mut inner, &keys, doc, records, now)?;
        }
        Ok(report)
    }

    /// THE resolution: the value `name` for `consumer`, or why not. Every denial is audited; a use is
    /// counted, and audited in aggregate.
    pub fn resolve(
        &self,
        name: &str,
        consumer: &Consumer,
        now: u64,
    ) -> Result<SecretString, VaultError> {
        self.resolve_entry(name, consumer, now)
            .map(|(secret, _)| secret)
    }

    fn resolve_entry(
        &self,
        name: &str,
        consumer: &Consumer,
        now: u64,
    ) -> Result<(SecretString, String), VaultError> {
        let mut inner = self.lock();
        let keys = inner.keys.clone().ok_or_else(|| self.unavailable(&inner))?;
        let (actor, node, cid) = match &consumer.on_behalf {
            OnBehalf::Agent { cid, node } => ("agent", node.clone(), Some(*cid)),
            OnBehalf::Owner { node } => (
                "owner",
                Some(node.clone().unwrap_or_else(|| LOCAL_NODE.to_string())),
                None,
            ),
            OnBehalf::Hub => ("hub", None, None),
        };
        let denial = |reason: &str| {
            let mut record = Record::new("deny", actor)
                .binding(&consumer.binding)
                .detail(reason);
            record.name = Some(name.to_string());
            if let Some(n) = &node {
                record = record.node(n);
            }
            if let Some(c) = cid {
                record = record.cid(c);
            }
            record
        };
        if node.is_none() && cid.is_some() {
            self.audit(
                &mut inner,
                &keys.audit,
                denial("the agent's node is revoked"),
                now,
            );
            return Err(VaultError::OutOfScope);
        }
        let chosen = {
            let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
            choose(&doc.entries, name, &consumer.binding, node.as_deref())
                .map(|index| doc.entries[index].clone())
        };
        let entry = match chosen {
            Ok(entry) => entry,
            Err(err) => {
                let reason = if err == VaultError::NotFound {
                    "no secret of that name is bound to this consumer"
                } else {
                    "the secret is not scoped to this node"
                };
                self.audit(&mut inner, &keys.audit, denial(reason), now);
                return Err(err);
            }
        };
        let value = match decrypt_current(&keys, &entry) {
            Ok(value) => value,
            Err(err) => {
                self.audit(
                    &mut inner,
                    &keys.audit,
                    Record::new("tamper", actor)
                        .entry(&entry.id, &entry.name)
                        .detail("value failed its check"),
                    now,
                );
                return Err(err);
            }
        };
        let text = std::str::from_utf8(&value)
            .map_err(|_| VaultError::Corrupt(format!("{name} is not text")))?;
        let secret = SecretString::from(text.to_string());
        if let Some(doc) = inner.doc.as_mut()
            && let Some(stored) = doc.entries.iter_mut().find(|e| e.id == entry.id)
        {
            stored.last_used_ms = now;
            stored.use_count += 1;
        }
        inner.usage_dirty = true;
        let key = (
            entry.id.clone(),
            consumer.binding.clone(),
            node.clone().unwrap_or_else(|| "-".into()),
        );
        let minute = now / MINUTE_MS;
        let stale = matches!(inner.uses.get(&key), Some((seen, _, _, _)) if *seen != minute);
        if stale && let Some(old) = inner.uses.remove(&key) {
            self.emit_use(&mut inner, &keys, key.clone(), old, now);
        }
        let slot =
            inner
                .uses
                .entry(key)
                .or_insert((minute, 0, entry.name.clone(), actor.to_string()));
        slot.1 += 1;
        Ok((secret, entry.id))
    }

    /// The consumers other modules registered, with the entry each node resolves to.
    pub fn consumers(&self, nodes: &[Node]) -> Result<Vec<Value>, VaultError> {
        let inner = self.lock();
        if inner.keys.is_none() {
            return Err(self.unavailable(&inner));
        }
        let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
        let mut node_ids = vec![LOCAL_NODE.to_string()];
        node_ids.extend(nodes.iter().filter(|n| !n.revoked).map(|n| n.id.clone()));
        Ok(inner
            .consumers
            .iter()
            .map(|reg| {
                let mut per_node = Map::new();
                for node in &node_ids {
                    let state = match choose(&doc.entries, &reg.wants, &reg.binding, Some(node.as_str())) {
                        Ok(index) => doc.entries[index].id.clone(),
                        Err(VaultError::OutOfScope) => "out_of_scope".into(),
                        Err(_) => "missing".into(),
                    };
                    per_node.insert(node.clone(), Value::from(state));
                }
                json!({ "binding": reg.binding, "label": reg.label, "wants": reg.wants, "per_node": per_node })
            })
            .collect())
    }

    /// Registers a consumer: its binding, a label, and the secret name it wants.
    pub fn register_consumer(&self, binding: &str, label: &str, wants: &str) {
        let mut inner = self.lock();
        inner
            .consumers
            .retain(|c| c.binding != binding || c.wants != wants);
        inner.consumers.push(Registration {
            binding: binding.into(),
            label: label.into(),
            wants: wants.into(),
        });
    }

    /// Removes every registration of `binding` (a removed provider, connector or MCP server). Returns how many went.
    pub fn unregister_consumer(&self, binding: &str) -> usize {
        let mut inner = self.lock();
        let before = inner.consumers.len();
        inner.consumers.retain(|c| c.binding != binding);
        before - inner.consumers.len()
    }

    /// Whether a secret `name` is bound to `binding` and usable on this machine, for a status view.
    /// Nothing is decrypted, counted or audited. Returns `secret`, `out_of_scope` (only other nodes
    /// hold it), `missing`, or `sealed` (the vault is sealed or tampered).
    pub fn presence(&self, name: &str, binding: &str) -> &'static str {
        let inner = self.lock();
        if inner.keys.is_none() {
            return "sealed";
        }
        let Some(doc) = inner.doc.as_ref() else {
            return "sealed";
        };
        match choose(&doc.entries, name, binding, Some(LOCAL_NODE)) {
            Ok(_) => "secret",
            Err(VaultError::OutOfScope) => "out_of_scope",
            Err(_) => "missing",
        }
    }

    /// Stores `value` as the plaza entry for `name` that `binding` reads: a new version of the plaza
    /// entry when there is one (its bindings gain `binding`), else a new plaza entry. Write-only:
    /// the value is never returned. Returns the entry id.
    pub fn put_plaza(
        &self,
        name: &str,
        binding: &str,
        label: &str,
        value: &SecretString,
        actor: &str,
        now: u64,
    ) -> Result<String, VaultError> {
        let found = {
            let inner = self.lock();
            if inner.keys.is_none() {
                return Err(self.unavailable(&inner));
            }
            let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
            doc.entries
                .iter()
                .find(|e| e.name == name && e.scope == Scope::Plaza)
                .map(|e| (e.id.clone(), e.bindings.clone()))
        };
        match found {
            Some((id, mut bindings)) => {
                self.rotate(&id, value, actor, now)?;
                if !bindings.iter().any(|b| b == binding) {
                    bindings.push(binding.to_string());
                    self.patch(&id, None, None, Some(bindings), actor, now)?;
                }
                Ok(id)
            }
            None => self.create(
                NewSecret {
                    name: name.to_string(),
                    kind: "api_key".into(),
                    label: label.to_string(),
                    value: SecretString::from(value.expose_secret().to_string()),
                    scope: Scope::Plaza,
                    bindings: vec![binding.to_string()],
                },
                actor,
                now,
            ),
        }
    }

    /// The hook of the migration (migrate.rs): one secret from a plaintext file, recorded as `migrate` with
    /// `detail` (`unbound` for an entry no consumer reads). No value is logged. Returns its id.
    pub fn migrate_entry(
        &self,
        new: NewSecret,
        detail: Option<&str>,
        now: u64,
    ) -> Result<String, VaultError> {
        let mut inner = self.lock();
        let (keys, mut doc) = self.active(&inner)?;
        let entry = build_entry(&keys, &doc, new, now)?;
        let (id, name) = (entry.id.clone(), entry.name.clone());
        doc.entries.push(entry);
        let mut record = Record::new("migrate", "hub").entry(&id, &name);
        if let Some(detail) = detail {
            record = record.detail(detail);
        }
        self.commit(&mut inner, &keys, doc, vec![record], now)?;
        Ok(id)
    }

    /// Every entry called `name`, whatever its scope, with what the migration and the connectors need: whether
    /// it is a plaza entry, and its bindings. Nothing is decrypted or audited.
    pub fn entries_named(&self, name: &str) -> Result<Vec<Named>, VaultError> {
        let inner = self.lock();
        if inner.keys.is_none() {
            return Err(self.unavailable(&inner));
        }
        let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
        Ok(doc
            .entries
            .iter()
            .filter(|e| e.name == name)
            .map(|e| Named {
                id: e.id.clone(),
                plaza: e.scope == Scope::Plaza,
                bindings: e.bindings.clone(),
            })
            .collect())
    }

    /// Whether `name` was deleted before: its tombstone keeps an env import from bringing it back.
    pub fn is_tombstoned(&self, name: &str) -> Result<bool, VaultError> {
        let inner = self.lock();
        if inner.keys.is_none() {
            return Err(self.unavailable(&inner));
        }
        let doc = inner.doc.as_ref().ok_or(VaultError::Uninitialized)?;
        Ok(doc.tombstones.iter().any(|t| t.name == name))
    }

    /// Records how many variables the env file holds (the status's `env_file_keys`).
    pub fn set_env_file_keys(&self, n: usize) {
        self.lock().env_file_keys = n;
    }

    /// The value a host's own consumer reads for `name` (the object store's keys, and the like). The vault's
    /// entry when one of that name is bound to `binding`; else the process environment, but only when no entry
    /// of that name is bound to this consumer. A sealed vault, or an entry this consumer may not read here, gives
    /// `None`: it never falls back to the environment.
    pub fn value_for(&self, name: &str, binding: &str, on_behalf: &OnBehalf) -> Option<String> {
        let named = self.entries_named(name).ok()?;
        if named
            .iter()
            .any(|e| e.bindings.iter().any(|b| b == binding))
        {
            let consumer = Consumer {
                binding: binding.to_string(),
                on_behalf: on_behalf.clone(),
            };
            return self
                .resolve(name, &consumer, apz_plaza::now_ms())
                .ok()
                .map(|secret| secret.expose_secret().to_string())
                .filter(|v| !v.is_empty());
        }
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }

    /// Called once after boot (state.rs). Registers the Hugging Face consumer, records the env file's key count,
    /// and on an unsealed vault runs the migration: the plaintext files and the env file's variables move into it
    /// (migrate.rs). Logs the env file line while the file still holds variables. Never fails the start.
    pub fn startup(
        &self,
        app: &crate::state::App,
        env_file: Option<&Path>,
        env: &BTreeMap<String, String>,
        now: u64,
    ) {
        self.register_consumer("hf", "Hugging Face token", "HF_TOKEN");
        let providers = app.providers.gateway().providers();
        let mut provider_refs = credential_refs(&providers);
        // The AWS pair has no single auth name, so it is listed per Bedrock-kind provider: an imported pair is
        // bound to every one of them, or to provider:bedrock when none exists yet.
        let mut aws = crate::providers::bedrock_bindings(&providers);
        if aws.is_empty() {
            aws.push(crate::providers::BEDROCK_BINDING.to_string());
        }
        for binding in aws {
            provider_refs.extend(
                crate::providers::BEDROCK_KEYS
                    .iter()
                    .map(|name| (binding.clone(), name_for(name))),
            );
        }
        // The variables both sources hold: the env file's, and the process environment's candidates it does not hold.
        let process = migrate::process_env_count(&provider_refs, env);
        self.set_env_file_keys(env.len() + process);
        if self.lock().keys.is_some() {
            migrate::run(
                self,
                &app.config.state_dir,
                app.hf.dir(),
                &provider_refs,
                env,
                now,
            );
        }
        if let Some(path) = env_file.filter(|_| !env.is_empty()) {
            eprintln!(
                "apid: keys in {} were moved into the vault; delete the file",
                path.display()
            );
        }
    }

    /// The status view: the state, the slots, the entry count, the nodes, and what the audit shows.
    pub fn status(&self, nodes: &[Node]) -> Value {
        let inner = self.lock();
        let state = if inner.doc.is_none() {
            "uninitialized"
        } else if inner.tamper.is_some() {
            "tampered"
        } else if inner.keys.is_some() {
            "unsealed"
        } else {
            "sealed"
        };
        // The slot that unlocked this session is marked; a sealed vault marks none.
        let unlocked = inner
            .keys
            .as_ref()
            .and_then(|keys| keys.unlocked_slot.clone());
        let (slot_views, entries) = match &inner.doc {
            Some(doc) => (
                doc.slots
                    .iter()
                    .map(|slot| {
                        let mut view = model::slot_view(slot);
                        view["unsealed_with"] =
                            json!(unlocked.as_deref() == Some(slot.id.as_str()));
                        view
                    })
                    .collect::<Vec<_>>(),
                doc.entries.len(),
            ),
            None => (Vec::new(), 0),
        };
        let mut node_list = vec![json!({ "id": LOCAL_NODE, "name": "this machine" })];
        node_list.extend(
            nodes
                .iter()
                .filter(|n| !n.revoked)
                .map(|n| json!({ "id": n.id, "name": n.name })),
        );
        json!({
            "state": state,
            "slots": slot_views,
            "entries": entries,
            "nodes": node_list,
            "audit_ok": inner.audit_ok,
            "rekeyed_ms": inner.rekeyed_ms,
            "tampered": inner.tamper,
            "env_file_keys": inner.env_file_keys,
        })
    }

    /// The database's `vault` component: the whole document (ciphertext and public metadata).
    pub fn snapshot(&self) -> String {
        match &self.lock().doc {
            Some(doc) => serde_json::to_string(doc).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// Restores the `vault` component at start. The vault is still sealed after it.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let doc: Document = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if doc.format != FORMAT {
            return Err(format!("vault format {} is not supported", doc.format));
        }
        let mut inner = self.lock();
        inner.doc = Some(doc);
        Ok(())
    }

    // ----- internals ------------------------------------------------------------------------

    /// The unlocked keys and a copy of the document, to change.
    fn active(&self, inner: &Inner) -> Result<(Arc<Keys>, Document), VaultError> {
        let keys = inner.keys.clone().ok_or_else(|| self.unavailable(inner))?;
        let doc = inner.doc.clone().ok_or(VaultError::Uninitialized)?;
        Ok((keys, doc))
    }

    fn unavailable(&self, inner: &Inner) -> VaultError {
        if let Some(reason) = &inner.tamper {
            VaultError::Tampered(reason.clone())
        } else if inner.doc.is_none() {
            VaultError::Uninitialized
        } else {
            VaultError::Sealed
        }
    }

    /// Signs and saves `doc`, then makes it current and chains `records` into the audit log.
    fn commit(
        &self,
        inner: &mut Inner,
        keys: &Keys,
        mut doc: Document,
        records: Vec<Record>,
        now: u64,
    ) -> Result<(), VaultError> {
        sign(keys, &mut doc);
        self.save(&doc)?;
        inner.doc = Some(doc);
        for record in records {
            self.audit(inner, &keys.audit, record, now);
        }
        Ok(())
    }

    /// Writes the document: the database's `vault` component when Postgres is configured, else
    /// `vault.json`. Never over a component that could not be read at start.
    fn save(&self, doc: &Document) -> Result<(), VaultError> {
        let text =
            serde_json::to_string_pretty(doc).map_err(|e| VaultError::Storage(e.to_string()))?;
        match &self.db {
            Some(db) => {
                if db.is_held("vault") {
                    return Err(VaultError::Storage(
                        "the stored vault could not be read at start; it is not overwritten".into(),
                    ));
                }
                db.store("vault", &text)
                    .map(|_| ())
                    .map_err(VaultError::Storage)
            }
            None => disk::write_atomic(&self.doc_path(), text.as_bytes())
                .map_err(|e| VaultError::Storage(format!("vault.json: {e}"))),
        }
    }

    /// Appends a record. A failed append is logged and marks the audit as not known to be ok.
    fn audit(&self, inner: &mut Inner, key: &[u8; 32], record: Record, now: u64) {
        if let Err(err) = audit::append(&self.audit_path(), &mut inner.head, key, &record, now) {
            eprintln!("vault: an audit line could not be written: {err}");
            inner.audit_ok = Some(false);
        }
    }

    fn flush_uses(&self, inner: &mut Inner, keys: &Keys, now: u64, all: bool) {
        let minute = now / MINUTE_MS;
        let due: Vec<UseKey> = inner
            .uses
            .iter()
            .filter(|(_, (seen, _, _, _))| all || *seen < minute)
            .map(|(k, _)| k.clone())
            .collect();
        for key in due {
            if let Some(value) = inner.uses.remove(&key) {
                self.emit_use(inner, keys, key, value, now);
            }
        }
    }

    fn emit_use(&self, inner: &mut Inner, keys: &Keys, key: UseKey, value: UseTally, now: u64) {
        let (entry, binding, node) = key;
        let (minute, count, name, actor) = value;
        let record = Record::new("use", &actor)
            .entry(&entry, &name)
            .binding(&binding)
            .node(&node)
            .detail(format!("{count} uses in minute {minute}"));
        self.audit(inner, &keys.audit, record, now);
    }
}

/// The saver thread: every 3 seconds, the aggregated use lines and the usage counters. A vault that
/// already reports a broken audit chain says so on the event stream once.
pub fn spawn_saver(app: Arc<crate::state::App>) {
    if app.vault.audit_ok() == Some(false) {
        app.bus
            .publish("vault.audit_broken", &json!({ "line": null }));
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(3));
            app.vault.flush(apz_plaza::now_ms());
        }
    });
}

// ----- free helpers ------------------------------------------------------------------------

/// The keys derived from the root key. The root is kept too, to wrap a new slot.
fn derive(root: &[u8; 32], vault_id: &str) -> Keys {
    Keys {
        root: Zeroizing::new(*root),
        wrap: crypto::subkey(root, vault_id, crypto::WRAP_INFO),
        index: crypto::subkey(root, vault_id, crypto::INDEX_INFO),
        audit: crypto::subkey(root, vault_id, crypto::AUDIT_INFO),
        unlock: None,
        unlocked_slot: None,
    }
}

/// Sets the document's index MAC.
fn sign(keys: &Keys, doc: &mut Document) {
    let view = crypto::canonical(&model::index_view(doc));
    doc.index_mac = crypto::mac_hex(&keys.index[..], &[view.as_bytes()]);
}

fn index_ok(keys: &Keys, doc: &Document) -> bool {
    let view = crypto::canonical(&model::index_view(doc));
    crypto::mac_matches(&keys.index[..], &[view.as_bytes()], &doc.index_mac)
}

fn new_id(prefix: &str) -> Result<String, VaultError> {
    let bytes = crypto::random_vec(8).map_err(VaultError::Storage)?;
    let text = crypto::base32(&bytes).to_lowercase();
    Ok(format!("{prefix}_{}", &text[..12]))
}

fn position(entries: &[Entry], id: &str) -> Result<usize, VaultError> {
    entries
        .iter()
        .position(|e| e.id == id)
        .ok_or(VaultError::NotFound)
}

fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=64).contains(&bytes.len())
        && bytes[0].is_ascii_uppercase()
        && bytes
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

fn valid_node(node: &str) -> bool {
    let digits = node.strip_prefix('n').unwrap_or("");
    !digits.is_empty() && digits.len() <= 6 && digits.bytes().all(|c| c.is_ascii_digit())
}

/// The `(binding, vault name)` pair of each provider that reads a named key (`Auth::Env` or `Auth::Secret`).
/// The env file's and the process environment's keys are bound through these pairs (see [`Store::startup`]).
pub(crate) fn credential_refs(providers: &[apz_gateway::ProviderConfig]) -> Vec<(String, String)> {
    providers
        .iter()
        .filter_map(|p| match &p.auth {
            apz_gateway::Auth::Secret(name) | apz_gateway::Auth::Env(name) => {
                Some((format!("provider:{}", p.id), name_for(name)))
            }
            _ => None,
        })
        .collect()
}

fn valid_binding(binding: &str) -> bool {
    let id_ok = |id: &str| {
        let bytes = id.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 64
            && bytes[0].is_ascii_alphanumeric()
            && bytes.iter().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
            })
    };
    // `egress`: the hub's egress credential rules (`egress_inject`), which name the secret they add.
    // `hub:<id>`: the hub's own background consumers (`hub:weft`, Weft's keys; `hub:git-platforms`, the platforms').
    // `project:<id>.<env>`: a project environment's secrets and deploy hook (`environments`).
    binding == "hf"
        || binding == "egress"
        || binding.split_once(':').is_some_and(|(prefix, id)| {
            matches!(
                prefix,
                "provider" | "connector" | "mcp" | "tool" | "hub" | "project"
            ) && id_ok(id)
        })
}

fn validate_bindings(list: &[String]) -> Result<Vec<String>, VaultError> {
    let mut out = Vec::with_capacity(list.len());
    for binding in list {
        if !valid_binding(binding) {
            return Err(VaultError::Invalid(format!(
                "{binding} is not a binding (provider:, connector:, mcp:, tool:, hub:, hf or egress)"
            )));
        }
        out.push(binding.clone());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn validate_scope(scope: Scope) -> Result<Scope, VaultError> {
    match scope {
        Scope::Plaza => Ok(Scope::Plaza),
        Scope::Nodes { mut nodes } => {
            if nodes.is_empty() {
                return Err(VaultError::Invalid(
                    "a nodes scope names at least one node".into(),
                ));
            }
            if let Some(bad) = nodes.iter().find(|n| !valid_node(n)) {
                return Err(VaultError::Invalid(format!("{bad} is not a node id")));
            }
            nodes.sort();
            nodes.dedup();
            Ok(Scope::Nodes { nodes })
        }
    }
}

fn scope_text(scope: &Scope) -> String {
    match scope {
        Scope::Plaza => "plaza".into(),
        Scope::Nodes { nodes } => nodes.join(","),
    }
}

fn validate_value(value: &str) -> Result<(), VaultError> {
    if value.is_empty() {
        return Err(VaultError::Invalid("the value is empty".into()));
    }
    if value.len() > MAX_VALUE_BYTES || value.contains('\0') {
        return Err(VaultError::Invalid(
            "the value is too long or holds a NUL byte".into(),
        ));
    }
    Ok(())
}

fn clean_label(label: &str) -> Result<String, VaultError> {
    let trimmed = label.trim();
    if trimmed.chars().count() > 200 {
        return Err(VaultError::Invalid(
            "the label is longer than 200 characters".into(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Refuses a second plaza entry for a name, and node sets that overlap another entry's.
fn check_scope(
    entries: &[Entry],
    name: &str,
    scope: &Scope,
    except: Option<&str>,
) -> Result<(), VaultError> {
    for other in entries
        .iter()
        .filter(|e| e.name == name && Some(e.id.as_str()) != except)
    {
        match (scope, &other.scope) {
            (Scope::Plaza, Scope::Plaza) => {
                return Err(VaultError::Conflict(format!(
                    "{name} already has a plaza entry; a name has one plaza entry"
                )));
            }
            (Scope::Nodes { nodes: wanted }, Scope::Nodes { nodes: have }) => {
                if let Some(node) = wanted.iter().find(|n| have.contains(n)) {
                    return Err(VaultError::Conflict(format!(
                        "{name} already has an entry for node {node}"
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The entry a consumer gets for `name` on `node` (`None`: the hub's background work).
fn choose(
    entries: &[Entry],
    name: &str,
    binding: &str,
    node: Option<&str>,
) -> Result<usize, VaultError> {
    let candidates: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name == name && e.bindings.iter().any(|b| b == binding))
        .map(|(i, _)| i)
        .collect();
    if candidates.is_empty() {
        return Err(VaultError::NotFound);
    }
    if let Some(node) = node {
        let in_node = candidates.iter().copied().find(|&i| {
            matches!(&entries[i].scope, Scope::Nodes { nodes } if nodes.iter().any(|x| x == node))
        });
        if let Some(index) = in_node {
            return Ok(index);
        }
    }
    candidates
        .iter()
        .copied()
        .find(|&i| entries[i].scope == Scope::Plaza)
        .ok_or(VaultError::OutOfScope)
}

fn value_aad(id: &str, version: u32, name: &str, scope: &Scope, bindings: &[String]) -> Vec<u8> {
    let v = version.to_string();
    crypto::aad(
        VALUE_LABEL,
        &[
            id,
            &v,
            name,
            &scope.canonical(),
            &model::bindings_canonical(bindings),
        ],
    )
}

fn dek_aad(id: &str, version: u32) -> Vec<u8> {
    let v = version.to_string();
    crypto::aad(DEK_LABEL, &[id, &v])
}

/// The hint of a value: its last 4 characters, for a value of 20 characters or more.
fn hint(value: &str) -> Option<Zeroizing<String>> {
    let chars: Vec<char> = value.chars().collect();
    (chars.len() >= HINT_MIN_CHARS)
        .then(|| Zeroizing::new(chars[chars.len() - 4..].iter().collect()))
}

fn hint_aad(id: &str, version: u32) -> Vec<u8> {
    let v = version.to_string();
    crypto::aad(HINT_LABEL, &[id, &v])
}

/// A hint sealed under K_index, as base64 `(nonce, ct)`. The AAD binds it to its entry and version.
fn seal_hint(
    keys: &Keys,
    id: &str,
    version: u32,
    hint: &str,
) -> Result<(String, String), VaultError> {
    let (nonce, ct) = crypto::seal(&keys.index, &hint_aad(id, version), hint.as_bytes())
        .map_err(VaultError::Storage)?;
    Ok((model::enc(&nonce), model::enc(&ct)))
}

/// A version's hint in clear. `None` when it has none, or when it fails its check: a hint is not a
/// value, so a failure writes no audit line.
fn open_hint(keys: &Keys, id: &str, version: &Version) -> Option<String> {
    let nonce = model::dec(version.hint_nonce.as_deref()?).ok()?;
    let ct = model::dec(version.hint_ct.as_deref()?).ok()?;
    let plain = crypto::open(&keys.index, &nonce, &hint_aad(id, version.v), &ct).ok()?;
    std::str::from_utf8(&plain).ok().map(str::to_string)
}

/// Re-seals a version's hint under the new index key (rekey). A hint that cannot be opened is dropped.
fn rehome_hint(old: &Keys, new: &Keys, id: &str, version: &mut Version) {
    let v = version.v;
    let sealed = open_hint(old, id, version).and_then(|plain| seal_hint(new, id, v, &plain).ok());
    (version.hint_nonce, version.hint_ct) = match sealed {
        Some((nonce, ct)) => (Some(nonce), Some(ct)),
        None => (None, None),
    };
}

/// An entry as the API shows it, with its hints in clear (the keys are unlocked to be here).
fn view_of(keys: &Keys, entry: &Entry) -> Value {
    model::entry_view(entry, |version| open_hint(keys, &entry.id, version))
}

/// One version: a fresh DEK seals the value, and the root's wrapping key wraps the DEK.
#[allow(clippy::too_many_arguments)]
fn seal_version(
    keys: &Keys,
    id: &str,
    version: u32,
    name: &str,
    scope: &Scope,
    bindings: &[String],
    value: &str,
    now: u64,
) -> Result<Version, VaultError> {
    let dek = crypto::random_key().map_err(VaultError::Storage)?;
    let (nonce, ct) = crypto::seal(
        &dek,
        &value_aad(id, version, name, scope, bindings),
        value.as_bytes(),
    )
    .map_err(VaultError::Storage)?;
    let (dek_nonce, dek_ct) =
        crypto::seal(&keys.wrap, &dek_aad(id, version), &dek[..]).map_err(VaultError::Storage)?;
    let (hint_nonce, hint_ct) = match hint(value) {
        Some(text) => {
            let (nonce, ct) = seal_hint(keys, id, version, &text)?;
            (Some(nonce), Some(ct))
        }
        None => (None, None),
    };
    Ok(Version {
        v: version,
        created_ms: now,
        nonce: model::enc(&nonce),
        ct: model::enc(&ct),
        dek_nonce: model::enc(&dek_nonce),
        dek_ct: model::enc(&dek_ct),
        fp: crypto::fingerprint(&keys.index, value.as_bytes()),
        hint_nonce,
        hint_ct,
        legacy_hint: None,
    })
}

/// A new entry at version 1, checked against the document (names, scopes, sizes).
fn build_entry(keys: &Keys, doc: &Document, new: NewSecret, now: u64) -> Result<Entry, VaultError> {
    if !valid_name(&new.name) {
        return Err(VaultError::Invalid(format!(
            "{} is not a name: upper case letters, digits and _, 2 to 64 characters",
            new.name
        )));
    }
    if !KINDS.contains(&new.kind.as_str()) {
        return Err(VaultError::Invalid(format!(
            "{} is not a kind (api_key, token, password, generic)",
            new.kind
        )));
    }
    validate_value(new.value.expose_secret())?;
    let scope = validate_scope(new.scope)?;
    let bindings = validate_bindings(&new.bindings)?;
    let label = clean_label(&new.label)?;
    check_scope(&doc.entries, &new.name, &scope, None)?;
    let id = new_id("sec")?;
    let version = seal_version(
        keys,
        &id,
        1,
        &new.name,
        &scope,
        &bindings,
        new.value.expose_secret(),
        now,
    )?;
    Ok(Entry {
        id,
        name: new.name,
        kind: new.kind,
        label,
        created_ms: now,
        updated_ms: now,
        created_by: "owner".into(),
        scope,
        bindings,
        current: 1,
        versions: vec![version],
        last_used_ms: 0,
        use_count: 0,
    })
}

/// Keeps the current version and the two before it, wiping the rest.
fn prune(entry: &mut Entry) {
    entry.versions.sort_by_key(|v| v.v);
    while entry.versions.len() > KEEP_VERSIONS {
        let mut old = entry.versions.remove(0);
        wipe(&mut old);
    }
}

fn wipe(version: &mut Version) {
    version.nonce.zeroize();
    version.ct.zeroize();
    version.dek_nonce.zeroize();
    version.dek_ct.zeroize();
    version.hint_nonce.zeroize();
    version.hint_ct.zeroize();
    version.legacy_hint.zeroize();
}

fn unwrap_dek(keys: &Keys, id: &str, version: &Version) -> Result<Key, VaultError> {
    let nonce = model::dec(&version.dek_nonce).map_err(VaultError::Corrupt)?;
    let ct = model::dec(&version.dek_ct).map_err(VaultError::Corrupt)?;
    let bytes = crypto::open(&keys.wrap, &nonce, &dek_aad(id, version.v), &ct)
        .map_err(|_| VaultError::Corrupt("a data key failed its check".into()))?;
    crypto::key_from(&bytes)
        .ok_or_else(|| VaultError::Corrupt("a data key has the wrong length".into()))
}

fn decrypt_current(keys: &Keys, entry: &Entry) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let version = entry
        .versions
        .iter()
        .find(|v| v.v == entry.current)
        .ok_or_else(|| VaultError::Corrupt(format!("{} has no current version", entry.name)))?;
    let dek = unwrap_dek(keys, &entry.id, version)?;
    let nonce = model::dec(&version.nonce).map_err(VaultError::Corrupt)?;
    let ct = model::dec(&version.ct).map_err(VaultError::Corrupt)?;
    crypto::open(
        &dek,
        &nonce,
        &value_aad(
            &entry.id,
            version.v,
            &entry.name,
            &entry.scope,
            &entry.bindings,
        ),
        &ct,
    )
    .map_err(|_| VaultError::Corrupt(format!("{} failed its check", entry.name)))
}

/// Re-encrypts one version's value under a new scope and bindings. The DEK is unchanged.
fn rewrap_value(
    keys: &Keys,
    id: &str,
    name: &str,
    old: (&Scope, &[String]),
    new: (&Scope, &[String]),
    version: &mut Version,
) -> Result<(), VaultError> {
    let dek = unwrap_dek(keys, id, version)?;
    let nonce = model::dec(&version.nonce).map_err(VaultError::Corrupt)?;
    let ct = model::dec(&version.ct).map_err(VaultError::Corrupt)?;
    let value = crypto::open(
        &dek,
        &nonce,
        &value_aad(id, version.v, name, old.0, old.1),
        &ct,
    )
    .map_err(|_| VaultError::Corrupt(format!("{name} failed its check")))?;
    let (nonce, ct) = crypto::seal(&dek, &value_aad(id, version.v, name, new.0, new.1), &value)
        .map_err(VaultError::Storage)?;
    version.nonce = model::enc(&nonce);
    version.ct = model::enc(&ct);
    Ok(())
}
