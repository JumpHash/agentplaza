//! The one-time move of the plaintext secrets into the vault (docs/design/vault.md, "Migration and the env file").
//! It runs on an unsealed start, after boot, in this order: `<state>/secrets.json`, `<state>/connector-secrets.json`,
//! the Hugging Face token file, then the env file's variables.
//!
//! A value is written as a `migrate` audit record (the env file's as `import`), read back through each of its
//! bindings and compared in constant time. Only when every value of a file verifies is the file overwritten with
//! zeros, fsynced and unlinked. A file with a value that does not verify, or an entry it cannot place, is kept and
//! logged. Nothing here logs a value.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use super::model::Scope;
use super::{Consumer, NewSecret, OnBehalf, Store, name_for, valid_name};
use crate::connectors;

const SECRETS: &str = "secrets.json";
const CONNECTOR_SECRETS: &str = "connector-secrets.json";
const HF_TOKEN_FILE: &str = "token";
const HF_NAME: &str = "HF_TOKEN";
const GARAGE_KEYS: [&str; 2] = ["APZ_S3_ACCESS_KEY", "APZ_S3_SECRET_KEY"];
const ZERO_CHUNK: usize = 64 * 1024;
const BOUND_LABEL: &str = "moved from a plaintext file";
const UNBOUND_LABEL: &str = "imported, not used by anything yet";

/// One value a file holds: its vault name, the value, and the bindings it gets.
struct Item {
    name: String,
    value: String,
    bindings: Vec<String>,
}

/// What a file parsed to: its values, and one reason per entry that was refused (a refused entry keeps the file).
#[derive(Default)]
struct Parsed {
    items: Vec<Item>,
    refused: Vec<String>,
}

/// Runs the migration. `provider_refs` is `(binding, vault name)` for each provider's credential reference.
/// Idempotent: a second run over the same files changes nothing.
///
/// Returns how many candidate variables the process environment holds that the env file does not (see
/// [`process_env_count`]).
pub(super) fn run(
    store: &Store,
    state_dir: &Path,
    hf_dir: &Path,
    provider_refs: &[(String, String)],
    env: &BTreeMap<String, String>,
    now: u64,
) -> usize {
    file(
        store,
        &state_dir.join(SECRETS),
        "secrets.json",
        now,
        |text| parse_secrets(text, provider_refs),
    );
    file(
        store,
        &state_dir.join(CONNECTOR_SECRETS),
        "connector-secrets.json",
        now,
        parse_connectors,
    );
    file(
        store,
        &hf_dir.join(HF_TOKEN_FILE),
        "the Hugging Face token file",
        now,
        parse_token,
    );
    import_env(store, env, "the env file", provider_refs, now);
    import_process_env(store, provider_refs, env, now)
}

/// The vault names the hub reads from the environment: each provider's credential, each connector field's `env`,
/// the Hugging Face token, and the object store's keys.
fn candidate_names(provider_refs: &[(String, String)]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = provider_refs.iter().map(|(_, name)| name.clone()).collect();
    names.extend(
        connectors::catalog()
            .into_iter()
            .flat_map(|c| c.fields.iter().map(|f| f.env.to_string())),
    );
    names.insert(HF_NAME.to_string());
    names.extend(GARAGE_KEYS.iter().map(|n| n.to_string()));
    names
}

/// The candidates that `lookup` finds non-empty (the process environment, or a fake in tests) and the env file does
/// not hold.
fn present_in(
    provider_refs: &[(String, String)],
    env: &BTreeMap<String, String>,
    lookup: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    candidate_names(provider_refs)
        .into_iter()
        .filter(|name| !env.contains_key(name))
        .filter_map(|name| lookup(&name).filter(|v| !v.is_empty()).map(|v| (name, v)))
        .collect()
}

/// How many candidates the process environment holds that the env file does not. Reads only the names; the
/// values are not kept.
pub(super) fn process_env_count(
    provider_refs: &[(String, String)],
    env: &BTreeMap<String, String>,
) -> usize {
    present_in(provider_refs, env, |name| std::env::var(name).ok()).len()
}

/// Imports the candidate variables of the process environment, by the rules of the env file (migrate.rs
/// `import_env`): once, plaza scope, unless an entry of that name exists or the name was deleted. Returns how many
/// candidates the process holds (imported or already in the vault).
pub(super) fn import_process_env(
    store: &Store,
    provider_refs: &[(String, String)],
    env: &BTreeMap<String, String>,
    now: u64,
) -> usize {
    import_from(
        store,
        provider_refs,
        env,
        |name| std::env::var(name).ok(),
        now,
    )
}

/// [`import_process_env`] with the lookup given: the process environment in the hub, a fake in the tests.
fn import_from(
    store: &Store,
    provider_refs: &[(String, String)],
    env: &BTreeMap<String, String>,
    lookup: impl Fn(&str) -> Option<String>,
    now: u64,
) -> usize {
    let found = present_in(provider_refs, env, lookup);
    import_env(store, &found, "the process environment", provider_refs, now);
    found.len()
}

/// Migrates one plaintext file: parses it, imports and verifies each value, then shreds the file when every
/// value is in the vault and nothing was refused. A missing file is nothing to do.
fn file(
    store: &Store,
    path: &Path,
    what: &str,
    now: u64,
    parse: impl FnOnce(&str) -> Result<Parsed, String>,
) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return,
        Err(err) => {
            eprintln!("apid: cannot read {what}: {err}; it is kept");
            return;
        }
    };
    if !bytes.is_empty() && bytes.iter().all(|b| *b == 0) {
        // An earlier shred stopped after the overwrite: the values were already moved.
        if fs::remove_file(path).is_ok() {
            eprintln!("apid: removed {what} left overwritten by an interrupted move");
        }
        return;
    }
    let parsed = match String::from_utf8(bytes)
        .map_err(|_| "it is not text".to_string())
        .and_then(|text| parse(&text))
    {
        Ok(parsed) => parsed,
        Err(reason) => {
            eprintln!("apid: {what} was not moved into the vault ({reason}); it is kept");
            return;
        }
    };
    let mut verified = parsed.refused.is_empty();
    for reason in &parsed.refused {
        eprintln!("apid: {what}: {reason}; the file is kept");
    }
    for item in &parsed.items {
        if let Err(reason) = import_item(store, item, now) {
            verified = false;
            eprintln!("apid: {what}: {}: {reason}; the file is kept", item.name);
        }
    }
    if !verified {
        return;
    }
    match shred(path) {
        Ok(()) => eprintln!("apid: {what} was moved into the vault and overwritten"),
        Err(err) => {
            eprintln!("apid: {what} was moved into the vault but could not be removed: {err}")
        }
    }
}

/// Stores one value (or finds it already stored) and verifies it: each binding reads it back; an unbound value is
/// read back from its ciphertext, since no consumer can read it yet.
fn import_item(store: &Store, item: &Item, now: u64) -> Result<(), String> {
    let named = store
        .entries_named(&item.name)
        .map_err(|err| err.message())?;
    let id = match named.iter().find(|entry| entry.plaza) {
        Some(entry) => {
            // The plaza entry already holds the value (an earlier run). Only its bindings can be missing.
            let missing: Vec<String> = item
                .bindings
                .iter()
                .filter(|b| !entry.bindings.contains(b))
                .cloned()
                .collect();
            if !missing.is_empty() {
                let mut bindings = entry.bindings.clone();
                bindings.extend(missing);
                store
                    .patch(&entry.id, None, None, Some(bindings), "hub", now)
                    .map_err(|err| err.message())?;
            }
            entry.id.clone()
        }
        None => {
            let unbound = item.bindings.is_empty();
            let new = NewSecret {
                name: item.name.clone(),
                kind: "api_key".into(),
                label: if unbound { UNBOUND_LABEL } else { BOUND_LABEL }.into(),
                value: SecretString::from(item.value.clone()),
                scope: Scope::Plaza,
                bindings: item.bindings.clone(),
            };
            store
                .migrate_entry(new, unbound.then_some("unbound"), now)
                .map_err(|err| err.message())?
        }
    };
    let verified = if item.bindings.is_empty() {
        read_back(store, &id, &item.value)
    } else {
        verify(store, item, now)
    };
    if verified {
        Ok(())
    } else {
        Err("the vault does not hold this value".into())
    }
}

/// The entry `id` decrypts to `value`, read from its ciphertext (no consumer, no audit).
fn read_back(store: &Store, id: &str, value: &str) -> bool {
    let inner = store.lock();
    let (Some(keys), Some(doc)) = (inner.keys.clone(), inner.doc.as_ref()) else {
        return false;
    };
    let Some(entry) = doc.entries.iter().find(|e| e.id == id) else {
        return false;
    };
    match super::decrypt_current(&keys, entry) {
        Ok(bytes) => std::str::from_utf8(&bytes).is_ok_and(|text| same(text, value)),
        Err(_) => false,
    }
}

/// Every binding of `item` resolves to the same value. Read back as the hub, so only plaza entries answer.
fn verify(store: &Store, item: &Item, now: u64) -> bool {
    !item.bindings.is_empty()
        && item.bindings.iter().all(|binding| {
            let consumer = Consumer {
                binding: binding.clone(),
                on_behalf: OnBehalf::Hub,
            };
            store
                .resolve(&item.name, &consumer, now)
                .map(|found| same(found.expose_secret(), &item.value))
                .unwrap_or(false)
        })
}

/// Equality in time that depends only on the lengths.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = u8::from(a.len() != b.len());
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

/// Overwrites the file with zeros of the same length, fsyncs it, and unlinks it.
fn shred(path: &Path) -> io::Result<()> {
    let len = fs::metadata(path)?.len();
    let mut file = OpenOptions::new().write(true).open(path)?;
    let zeros = vec![0u8; ZERO_CHUNK];
    let mut left = len;
    while left > 0 {
        let n = left.min(ZERO_CHUNK as u64) as usize;
        file.write_all(&zeros[..n])?;
        left -= n as u64;
    }
    file.sync_all()?;
    drop(file);
    fs::remove_file(path)
}

/// The text of a JSON string value; anything else is not a credential.
fn text_of(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

/// `{name: value}`. Each name is a vault name; its bindings are the consumers that read it. A name nothing reads
/// is imported unbound (no bindings), so no plaintext is left behind.
fn parse_secrets(text: &str, provider_refs: &[(String, String)]) -> Result<Parsed, String> {
    let raw: BTreeMap<String, Value> = serde_json::from_str(text)
        .map_err(|_| "it is not a JSON object of names and values".to_string())?;
    let mut parsed = Parsed::default();
    for (key, value) in raw {
        let name = name_for(&key);
        let Some(value) = text_of(&value) else {
            parsed
                .refused
                .push(format!("{name}: the value is not text"));
            continue;
        };
        if value.is_empty() {
            continue;
        }
        if !valid_name(&name) {
            parsed.refused.push(format!("{name}: not a valid name"));
            continue;
        }
        let bindings = bindings_for(&name, provider_refs);
        parsed.items.push(Item {
            name,
            value,
            bindings,
        });
    }
    Ok(parsed)
}

/// `{connector: {field: value}}`, or the old `{connector: "key"}` (the connector's main key). A known field's
/// value is the field's `env` name, bound to `connector:<id>`. An unknown connector or field is imported under
/// its own name, unbound.
fn parse_connectors(text: &str) -> Result<Parsed, String> {
    let raw: BTreeMap<String, Value> = serde_json::from_str(text)
        .map_err(|_| "it is not a JSON object of connectors".to_string())?;
    let mut parsed = Parsed::default();
    for (id, entry) in raw {
        let connector = connectors::find(&id);
        let pairs: Vec<(String, Value)> = match entry {
            Value::String(key) => {
                let field = connector
                    .and_then(connectors::main_field)
                    .map_or_else(|| id.clone(), |f| f.name.to_string());
                vec![(field, Value::String(key))]
            }
            Value::Object(map) => map.into_iter().collect(),
            _ => {
                parsed
                    .refused
                    .push(format!("{id}: not a connector's fields"));
                continue;
            }
        };
        for (field_name, value) in pairs {
            let Some(text) = text_of(&value) else {
                parsed
                    .refused
                    .push(format!("{id}.{field_name}: the value is not text"));
                continue;
            };
            if text.is_empty() {
                continue;
            }
            match connector.and_then(|c| c.fields.iter().find(|f| f.name == field_name)) {
                Some(field) => parsed.items.push(Item {
                    name: field.env.to_string(),
                    value: text,
                    bindings: vec![format!("connector:{id}")],
                }),
                None => {
                    let name = name_for(&field_name);
                    if !valid_name(&name) {
                        parsed
                            .refused
                            .push(format!("{id}.{field_name}: not a valid name"));
                        continue;
                    }
                    parsed.items.push(Item {
                        name,
                        value: text,
                        bindings: Vec::new(),
                    });
                }
            }
        }
    }
    Ok(parsed)
}

/// The Hugging Face token file: one line, no spaces (the same rule `set_token` keeps). Bound to `hf`.
fn parse_token(text: &str) -> Result<Parsed, String> {
    let token = text.trim();
    let mut parsed = Parsed::default();
    if token.is_empty() {
        return Ok(parsed);
    }
    if token.len() > 512
        || token
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err("the token is not one line with no spaces".into());
    }
    parsed.items.push(Item {
        name: HF_NAME.into(),
        value: token.to_string(),
        bindings: vec!["hf".into()],
    });
    Ok(parsed)
}

/// The bindings that read the vault name `name`: the providers that reference it, the connector fields with
/// that env name, the Hugging Face token, and the object store's keys.
fn bindings_for(name: &str, provider_refs: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = provider_refs
        .iter()
        .filter(|(_, vault)| vault == name)
        .map(|(binding, _)| binding.clone())
        .collect();
    for connector in connectors::catalog() {
        if connector.fields.iter().any(|f| f.env == name) {
            out.push(format!("connector:{}", connector.id));
        }
    }
    if name == HF_NAME {
        out.push("hf".into());
    }
    if GARAGE_KEYS.contains(&name) {
        out.push("tool:garage".into());
    }
    out.sort();
    out.dedup();
    out
}

/// The env file's variables (or the process environment's, named by `source`) that are not in the vault yet and
/// were not deleted. Each is imported once, plaza scope, with the bindings that read it (possibly none). The
/// source itself is never changed here.
fn import_env(
    store: &Store,
    env: &BTreeMap<String, String>,
    source: &str,
    provider_refs: &[(String, String)],
    now: u64,
) {
    for (name, value) in env {
        if value.is_empty() || !valid_name(name) {
            continue;
        }
        match store.entries_named(name) {
            Ok(named) if named.is_empty() => {}
            Ok(_) => continue,
            Err(err) => {
                eprintln!(
                    "apid: a variable from {source} was not imported: {}",
                    err.message()
                );
                return;
            }
        }
        match store.is_tombstoned(name) {
            Ok(false) => {}
            Ok(true) => continue,
            Err(err) => {
                eprintln!(
                    "apid: a variable from {source} was not imported: {}",
                    err.message()
                );
                return;
            }
        }
        let new = NewSecret {
            name: name.clone(),
            kind: "generic".into(),
            label: "imported from the env file".into(),
            value: SecretString::from(value.clone()),
            scope: Scope::Plaza,
            bindings: bindings_for(name, provider_refs),
        };
        if let Err(err) = store.import_entry(new, "hub", now) {
            eprintln!(
                "apid: {name} from {source} was not imported: {}",
                err.message()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;
    const OPENAI: &str = "sk-SENTINEL-MIG-1001";
    const CONN: &str = "SENTINEL-CONN-4417";
    const HF: &str = "hf_SENTINEL-HF-2290";
    const ENV: &str = "SENTINEL-ENV-7735";
    const TOMB: &str = "SENTINEL-TOMB-3316";
    const PERPLEX: &str = "SENTINEL-PPLX-5508";
    const UNBOUND: &str = "SENTINEL-UNB-6612";
    const PROC: &str = "SENTINEL-PROC-1";
    const NODE: &str = "SENTINEL-NODE-2";

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn open(dir: &Path) -> Store {
        let store = Store::open_in(dir, &dir.join("vault.key"));
        store.boot(NOW).expect("boot");
        store
    }

    /// Every file under `dir`, read as bytes.
    fn all_bytes(dir: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(all_bytes(&path));
            } else if let Ok(bytes) = fs::read(&path) {
                out.extend(bytes);
            }
        }
        out
    }

    fn contains(haystack: &[u8], needle: &str) -> bool {
        haystack
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
    }

    fn resolve_for(store: &Store, name: &str, binding: &str) -> Option<String> {
        let consumer = Consumer {
            binding: binding.into(),
            on_behalf: OnBehalf::Hub,
        };
        store
            .resolve(name, &consumer, NOW)
            .ok()
            .map(|s| s.expose_secret().to_string())
    }

    fn refs() -> Vec<(String, String)> {
        vec![
            ("provider:openai".into(), "OPENAI_KEY".into()),
            ("provider:perplexity".into(), "PERPLEXITY_API_KEY".into()),
        ]
    }

    fn env_map() -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("PERPLEXITY_API_KEY".to_string(), PERPLEX.to_string());
        env.insert("SOME_EXTRA_KEY".to_string(), ENV.to_string());
        env.insert("lower".to_string(), "not valid".to_string());
        env.insert("TOMBSTONED_KEY".to_string(), TOMB.to_string());
        env
    }

    /// Writes the three plaintext files and an env map, runs the migration, and checks the outcome.
    #[test]
    fn migration_moves_every_file_and_leaves_no_plaintext() {
        let dir = scratch("moves");
        let state = dir.join("state");
        let hf = state.join("foundry").join("hf");
        fs::create_dir_all(&hf).unwrap();
        fs::write(
            state.join(SECRETS),
            format!(r#"{{"openai-key":"{OPENAI}","unused-thing":"x"}}"#),
        )
        .unwrap();
        fs::write(
            state.join(CONNECTOR_SECRETS),
            format!(r#"{{"firecrawl":"{CONN}","telegram":{{"bot_token":"123:{CONN}"}}}}"#),
        )
        .unwrap();
        fs::write(hf.join(HF_TOKEN_FILE), format!("{HF}\n")).unwrap();
        let store = open(&state);
        // A tombstone for TOMBSTONED_KEY: created, then deleted, before the env file is read.
        let id = store
            .create(
                NewSecret {
                    name: "TOMBSTONED_KEY".into(),
                    kind: "generic".into(),
                    label: "t".into(),
                    value: SecretString::from(TOMB.to_string()),
                    scope: Scope::Plaza,
                    bindings: vec![],
                },
                "owner",
                NOW,
            )
            .unwrap();
        store.delete(&id, "owner", NOW).unwrap();

        run(&store, &state, &hf, &refs(), &env_map(), NOW);

        assert!(
            !state.join(CONNECTOR_SECRETS).exists(),
            "connector-secrets.json is shredded"
        );
        assert!(
            !hf.join(HF_TOKEN_FILE).exists(),
            "the HF token file is shredded"
        );
        assert!(
            !state.join(SECRETS).exists(),
            "secrets.json is gone: its unused entry is imported unbound"
        );
        let unused = store.entries_named("UNUSED_THING").unwrap();
        assert!(unused.len() == 1 && unused[0].plaza && unused[0].bindings.is_empty());
        assert_eq!(
            resolve_for(&store, "OPENAI_KEY", "provider:openai").as_deref(),
            Some(OPENAI)
        );
        assert_eq!(
            resolve_for(&store, "FIRECRAWL_API_KEY", "connector:firecrawl").as_deref(),
            Some(CONN)
        );
        assert_eq!(
            resolve_for(&store, "TELEGRAM_BOT_TOKEN", "connector:telegram").as_deref(),
            Some("123:SENTINEL-CONN-4417")
        );
        assert_eq!(resolve_for(&store, "HF_TOKEN", "hf").as_deref(), Some(HF));
        assert_eq!(
            resolve_for(&store, "PERPLEXITY_API_KEY", "provider:perplexity").as_deref(),
            Some(PERPLEX)
        );
        assert_eq!(
            resolve_for(&store, "SOME_EXTRA_KEY", "provider:none").as_deref(),
            None,
            "an env key with no binding is stored, invisible to consumers"
        );
        assert_eq!(
            resolve_for(&store, "TOMBSTONED_KEY", "provider:none"),
            None,
            "a deleted name is not imported again"
        );
        assert!(store.is_tombstoned("TOMBSTONED_KEY").unwrap());
        assert_eq!(
            store.entries_named("lower").unwrap().len(),
            0,
            "an invalid env name is not imported"
        );
    }

    #[test]
    fn a_second_run_changes_nothing_and_no_value_is_left_on_disk() {
        let dir = scratch("twice");
        let state = dir.join("state");
        let hf = state.join("foundry").join("hf");
        fs::create_dir_all(&hf).unwrap();
        fs::write(
            state.join(CONNECTOR_SECRETS),
            format!(r#"{{"firecrawl":{{"api_key":"{CONN}"}}}}"#),
        )
        .unwrap();
        fs::write(hf.join(HF_TOKEN_FILE), HF).unwrap();
        fs::write(
            state.join(SECRETS),
            format!(r#"{{"openai-key":"{OPENAI}"}}"#),
        )
        .unwrap();
        let store = open(&state);
        run(&store, &state, &hf, &refs(), &BTreeMap::new(), NOW);
        assert!(
            !state.join(SECRETS).exists(),
            "secrets.json is shredded when every value is placed"
        );
        let before = store.list().unwrap().len();
        let mut env = BTreeMap::new();
        env.insert("PERPLEXITY_API_KEY".to_string(), PERPLEX.to_string());
        run(&store, &state, &hf, &refs(), &env, NOW);
        run(&store, &state, &hf, &refs(), &env, NOW);
        assert_eq!(
            store.list().unwrap().len(),
            before + 1,
            "only the env key is new, and only once"
        );
        assert_eq!(
            resolve_for(&store, "OPENAI_KEY", "provider:openai").as_deref(),
            Some(OPENAI)
        );
        assert!(
            !contains(&all_bytes(&state), OPENAI),
            "no plaintext of a moved file is left"
        );
        assert!(!contains(&all_bytes(&state), CONN));
        assert!(!contains(&all_bytes(&state), HF));
    }

    #[test]
    fn a_file_whose_value_is_not_in_the_vault_is_kept() {
        let dir = scratch("kept");
        let state = dir.join("state");
        let hf = state.join("foundry").join("hf");
        fs::create_dir_all(&hf).unwrap();
        // A plaza entry already holds another value for the name: the file's value does not verify, so it stays.
        fs::write(
            state.join(CONNECTOR_SECRETS),
            format!(r#"{{"firecrawl":"{CONN}"}}"#),
        )
        .unwrap();
        let store = open(&state);
        store
            .create(
                NewSecret {
                    name: "FIRECRAWL_API_KEY".into(),
                    kind: "api_key".into(),
                    label: "x".into(),
                    value: SecretString::from("SENTINEL-OTHER-0001".to_string()),
                    scope: Scope::Plaza,
                    bindings: vec!["connector:firecrawl".into()],
                },
                "owner",
                NOW,
            )
            .unwrap();
        run(&store, &state, &hf, &refs(), &BTreeMap::new(), NOW);
        assert!(
            state.join(CONNECTOR_SECRETS).exists(),
            "a file that does not verify is kept"
        );
    }

    #[test]
    fn an_unbound_value_is_moved_too_and_audited_as_unbound() {
        let dir = scratch("unbound");
        let state = dir.join("state");
        let hf = state.join("foundry").join("hf");
        fs::create_dir_all(&hf).unwrap();
        fs::write(
            state.join(SECRETS),
            format!(r#"{{"openai-key":"{OPENAI}","unbound-fake":"{UNBOUND}"}}"#),
        )
        .unwrap();
        let store = open(&state);
        run(&store, &state, &hf, &refs(), &BTreeMap::new(), NOW);
        assert!(!state.join(SECRETS).exists(), "the file is gone");
        let named = store.entries_named("UNBOUND_FAKE").unwrap();
        assert!(named.len() == 1 && named[0].plaza && named[0].bindings.is_empty());
        let view = store
            .list()
            .unwrap()
            .into_iter()
            .find(|v| v["name"] == "UNBOUND_FAKE")
            .unwrap();
        assert_eq!(view["label"], "imported, not used by anything yet");
        assert!(store.audit_page(0, usize::MAX).unwrap().iter().any(|v| {
            v["event"] == "migrate" && v["name"] == "UNBOUND_FAKE" && v["detail"] == "unbound"
        }));
        let bytes = all_bytes(&state);
        assert!(
            !contains(&bytes, OPENAI) && !contains(&bytes, UNBOUND),
            "no plaintext is left under the state dir"
        );
        assert_eq!(
            resolve_for(&store, "OPENAI_KEY", "provider:openai").as_deref(),
            Some(OPENAI)
        );
    }

    #[test]
    fn an_env_file_key_is_not_put_in_the_process_environment() {
        let dir = scratch("envfile");
        let state = dir.join("state");
        let hf = state.join("foundry").join("hf");
        fs::create_dir_all(&hf).unwrap();
        let store = open(&state);
        let name = "SENTINEL_ENVFILE_KEY_3320";
        let mut env = BTreeMap::new();
        env.insert(name.to_string(), ENV.to_string());
        run(&store, &state, &hf, &refs(), &env, NOW);
        assert!(
            std::env::var(name).is_err(),
            "the env file's variable stays in the file, not in the process"
        );
        assert_eq!(resolve_for(&store, name, "provider:none"), None);
        assert_eq!(
            store.entries_named(name).unwrap().len(),
            1,
            "but it is imported to the vault"
        );
    }

    #[test]
    fn a_process_variable_is_imported_once_and_a_node_entry_beats_the_plaza_one() {
        let dir = scratch("process");
        let state = dir.join("state");
        let store = open(&state);
        let name = "SENTINEL_PROC_KEY_9921";
        let refs = vec![("provider:proc".to_string(), name.to_string())];
        let process = |n: &str| (n == name).then(|| PROC.to_string());
        assert_eq!(
            import_from(&store, &refs, &BTreeMap::new(), process, NOW),
            1
        );
        assert_eq!(
            import_from(&store, &refs, &BTreeMap::new(), process, NOW),
            1,
            "still present, not imported twice"
        );
        assert_eq!(store.entries_named(name).unwrap().len(), 1);
        assert_eq!(
            resolve_for(&store, name, "provider:proc").as_deref(),
            Some(PROC)
        );
        store
            .create(
                NewSecret {
                    name: name.into(),
                    kind: "api_key".into(),
                    label: "node".into(),
                    value: SecretString::from(NODE.to_string()),
                    scope: Scope::Nodes {
                        nodes: vec!["n3".into()],
                    },
                    bindings: vec!["provider:proc".into()],
                },
                "owner",
                NOW,
            )
            .unwrap();
        let read = |node: &str| {
            let consumer = Consumer {
                binding: "provider:proc".into(),
                on_behalf: OnBehalf::Owner {
                    node: Some(node.into()),
                },
            };
            store
                .resolve(name, &consumer, NOW)
                .map(|s| s.expose_secret().to_string())
                .ok()
        };
        assert_eq!(read("n3").as_deref(), Some(NODE));
        assert_eq!(read("n0").as_deref(), Some(PROC));
    }

    #[test]
    fn unregistering_a_binding_removes_only_its_registrations() {
        let dir = scratch("unregister");
        let store = open(&dir);
        store.register_consumer("provider:gone", "Gone", "GONE_KEY");
        store.register_consumer("provider:gone", "Gone", "OTHER_KEY");
        store.register_consumer("hf", "Hugging Face token", "HF_TOKEN");
        assert_eq!(store.unregister_consumer("provider:gone"), 2);
        let left = store.consumers(&[]).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0]["binding"], "hf");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_aws_pair_from_the_env_file_and_the_process_is_bound_to_bedrock_and_served() {
        const KEY: &str = "AKIAFAKESENTINEL0001";
        const SECRET: &str = "SENTINEL-AWS-SECRET-7";
        let dir = scratch("aws");
        let store = open(&dir);
        let refs: Vec<(String, String)> = crate::providers::BEDROCK_KEYS
            .iter()
            .map(|name| {
                (
                    crate::providers::BEDROCK_BINDING.to_string(),
                    name_for(name),
                )
            })
            .collect();
        // The env file holds the access key id.
        let file = dir.join("apid.env");
        fs::write(&file, format!("AWS_ACCESS_KEY_ID={KEY}\n")).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        let env: BTreeMap<String, String> = crate::vault::dotenv::parse(&text)
            .unwrap()
            .into_iter()
            .map(|line| (line.name, line.value.as_str().to_string()))
            .collect();
        import_env(&store, &env, "the env file", &refs, NOW);
        // The process holds the secret key (the fake lookup stands in for it).
        let process = |name: &str| (name == "AWS_SECRET_ACCESS_KEY").then(|| SECRET.to_string());
        assert_eq!(import_from(&store, &refs, &env, process, NOW), 1);
        assert_eq!(
            resolve_for(&store, "AWS_ACCESS_KEY_ID", "provider:bedrock").as_deref(),
            Some(KEY)
        );
        assert_eq!(
            resolve_for(&store, "AWS_SECRET_ACCESS_KEY", "provider:bedrock").as_deref(),
            Some(SECRET)
        );
        assert_eq!(
            resolve_for(&store, "AWS_ACCESS_KEY_ID", "provider:openai"),
            None,
            "bound to bedrock only"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_imported_aws_pair_is_bound_to_every_bedrock_provider() {
        const KEY: &str = "AKIAFAKESENTINEL0002";
        let dir = scratch("aws-each");
        let store = open(&dir);
        let refs: Vec<(String, String)> = ["provider:aws-a", "provider:aws-b"]
            .iter()
            .flat_map(|binding| {
                crate::providers::BEDROCK_KEYS
                    .iter()
                    .map(move |name| (binding.to_string(), name_for(name)))
            })
            .collect();
        let env: BTreeMap<String, String> =
            BTreeMap::from([("AWS_ACCESS_KEY_ID".to_string(), KEY.to_string())]);
        import_env(&store, &env, "the env file", &refs, NOW);
        assert_eq!(
            resolve_for(&store, "AWS_ACCESS_KEY_ID", "provider:aws-a").as_deref(),
            Some(KEY)
        );
        assert_eq!(
            resolve_for(&store, "AWS_ACCESS_KEY_ID", "provider:aws-b").as_deref(),
            Some(KEY)
        );
        assert_eq!(
            resolve_for(&store, "AWS_ACCESS_KEY_ID", "provider:bedrock"),
            None,
            "no provider `bedrock` exists here"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zeroed_leftover_is_removed() {
        let dir = scratch("zeroed");
        let state = dir.join("state");
        let hf = state.join("foundry").join("hf");
        fs::create_dir_all(&hf).unwrap();
        fs::write(state.join(SECRETS), vec![0u8; 40]).unwrap();
        let store = open(&state);
        run(&store, &state, &hf, &refs(), &BTreeMap::new(), NOW);
        assert!(!state.join(SECRETS).exists());
    }
}
