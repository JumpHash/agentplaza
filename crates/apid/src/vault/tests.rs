//! The vault's tests (the store and its routes). Temp directories only; every vault gets its own
//! key file in its temp directory. Crypto, slots, audit, disk and dotenv have their own tests beside them.

use std::path::{Path, PathBuf};

use apz_http::{Request, Response};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};

use super::model::Scope;
use super::routes::{self, Ctx};
use super::{Consumer, NewSecret, OnBehalf, Store, Unlock, VaultError, node_for_cid};
use crate::events::Bus;
use crate::nodes::{self, Node};

const NOW: u64 = 1_700_000_000_000;
/// A value nobody may ever see in an output: routes, events, the audit file, the vault file.
const SENTINEL: &str = "SENTINEL-9f3c2a7e-do-not-print-Q7~!";
/// The sentinel's last four characters: its hint. An unsealed view shows it, and nothing else may.
const HINT: &str = "Q7~!";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apid-vault-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A vault in a fresh directory, booted: it has a key file slot and is unsealed.
fn fresh(name: &str) -> (Store, PathBuf) {
    let dir = scratch(name);
    let key = dir.join("config").join("vault.key");
    let store = Store::open(&dir, &key, None);
    store.boot(NOW).expect("boot");
    (store, dir)
}

fn reopen(dir: &Path) -> Store {
    let store = Store::open(dir, &dir.join("config").join("vault.key"), None);
    store.boot(NOW).expect("boot");
    store
}

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_string())
}

fn new(name: &str, value: &str, scope: Scope, bindings: &[&str]) -> NewSecret {
    NewSecret {
        name: name.into(),
        kind: "api_key".into(),
        label: String::new(),
        value: secret(value),
        scope,
        bindings: bindings.iter().map(|b| b.to_string()).collect(),
    }
}

fn nodes_scope(ids: &[&str]) -> Scope {
    Scope::Nodes {
        nodes: ids.iter().map(|s| s.to_string()).collect(),
    }
}

fn node(id: &str, base: u64, count: u64, revoked: bool) -> Node {
    serde_json::from_value(json!({
        "id": id,
        "name": id,
        "public_key": "",
        "cid_base": base,
        "cid_count": count,
        "joined_ms": 1,
        "last_seen_ms": NOW,
        "revoked": revoked,
    }))
    .expect("a node")
}

fn status_state(store: &Store) -> String {
    store.status(&[])["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn hub(binding: &str) -> Consumer {
    Consumer {
        binding: binding.into(),
        on_behalf: OnBehalf::Hub,
    }
}

fn keyfile_slot(store: &Store) -> String {
    let status = store.status(&[]);
    status["slots"]
        .as_array()
        .and_then(|s| s.iter().find(|x| x["kind"] == "keyfile"))
        .and_then(|x| x["id"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn wrap_cts(dir: &Path) -> (Vec<String>, Vec<String>) {
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("vault.json")).expect("vault.json"))
            .expect("json");
    let slots = doc["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .map(|s| s["wrap"]["ct"].as_str().unwrap_or_default().to_string())
        .collect();
    let deks = doc["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .flat_map(|e| e["versions"].as_array().cloned().unwrap_or_default())
        .map(|v| v["dek_ct"].as_str().unwrap_or_default().to_string())
        .collect();
    (slots, deks)
}

// ----- the spec's tests --------------------------------------------------------------------

#[test]
fn a_scope_or_binding_edited_in_the_file_is_tampered() {
    let (store, dir) = fresh("tamper");
    store
        .create(
            new(
                "ANTHROPIC_API_KEY",
                "value-000000000000000001",
                Scope::Plaza,
                &["provider:anthropic"],
            ),
            "owner",
            NOW,
        )
        .expect("created");
    drop(store);
    let path = dir.join("vault.json");
    let text = std::fs::read_to_string(&path).expect("read");
    std::fs::write(
        &path,
        text.replacen("provider:anthropic", "provider:openai", 1),
    )
    .expect("edited");
    let store = reopen(&dir);
    assert_eq!(status_state(&store), "tampered");
    assert!(matches!(store.list(), Err(VaultError::Tampered(_))));
    let consumer = hub("provider:openai");
    assert!(
        store.resolve("ANTHROPIC_API_KEY", &consumer, NOW).is_err(),
        "nothing resolves while tampered"
    );
}

#[test]
fn a_flipped_byte_in_a_keyslot_fails_to_unseal() {
    let (store, dir) = fresh("flip-slot");
    store.seal("owner", NOW).expect("sealed");
    drop(store);
    let path = dir.join("vault.json");
    let text = std::fs::read_to_string(&path).expect("read");
    let mut doc: Value = serde_json::from_str(&text).expect("json");
    let ct = doc["slots"][0]["wrap"]["ct"]
        .as_str()
        .expect("ct")
        .to_string();
    let mut bytes = super::model::dec(&ct).expect("base64");
    bytes[0] ^= 1;
    doc["slots"][0]["wrap"]["ct"] = Value::from(super::model::enc(&bytes));
    // The index MAC covers the slot without its wrap, so only the AEAD notices this change.
    std::fs::write(&path, doc.to_string()).expect("edited");
    let store = reopen(&dir);
    assert_eq!(status_state(&store), "sealed");
    assert_eq!(
        store.unseal(Unlock::Keyfile, "owner", NOW),
        Err(VaultError::BadSecret)
    );
}

#[test]
fn a_wrong_passphrase_fails_and_is_rate_limited() {
    let (store, _dir) = fresh("lockout");
    store
        .add_passphrase(&secret("correct horse battery"), "owner", NOW)
        .expect("slot");
    store.seal("owner", NOW).expect("sealed");
    for _ in 0..4 {
        assert_eq!(
            store.unseal(
                Unlock::Passphrase(secret("wrong passphrase!!")),
                "owner",
                NOW
            ),
            Err(VaultError::BadSecret)
        );
    }
    // The fifth failure within a minute starts the lockout.
    assert!(matches!(
        store.unseal(
            Unlock::Passphrase(secret("wrong passphrase!!")),
            "owner",
            NOW
        ),
        Err(VaultError::Locked { .. })
    ));
    assert!(matches!(
        store.unseal(
            Unlock::Passphrase(secret("correct horse battery")),
            "owner",
            NOW + 1000
        ),
        Err(VaultError::Locked { .. })
    ));
    store
        .unseal(
            Unlock::Passphrase(secret("correct horse battery")),
            "owner",
            NOW + 600_001,
        )
        .expect("unsealed after the lockout");
    assert_eq!(status_state(&store), "unsealed");
}

#[test]
fn slots_add_and_remove_and_the_last_slot_is_kept() {
    let (store, dir) = fresh("slots");
    let passphrase = store
        .add_passphrase(&secret("correct horse battery"), "owner", NOW)
        .expect("added");
    let keyfile = keyfile_slot(&store);
    assert!(
        matches!(
            store.add_keyfile("owner", NOW),
            Err(VaultError::Conflict(_))
        ),
        "one key file slot at a time"
    );
    store.remove_slot(&keyfile, "owner", NOW).expect("removed");
    assert!(
        !dir.join("config").join("vault.key").exists(),
        "removing the key-file slot destroys the key file"
    );
    assert_eq!(
        store.remove_slot(&passphrase, "owner", NOW),
        Err(VaultError::LastSlot)
    );
    assert!(matches!(
        store.remove_slot("slt_missing", "owner", NOW),
        Err(VaultError::LastSlot | VaultError::NotFound)
    ));
}

#[test]
fn the_recovery_code_opens_the_vault_and_a_new_code_replaces_the_old() {
    let (store, _dir) = fresh("recovery");
    let first = store.make_recovery("owner", NOW).expect("made");
    assert_eq!(first.replace('-', "").len(), 52);
    let second = store.make_recovery("owner", NOW).expect("made again");
    store.seal("owner", NOW).expect("sealed");
    assert_eq!(
        store.unseal(Unlock::Recovery(secret(&first)), "owner", NOW),
        Err(VaultError::BadSecret),
        "the old code is gone"
    );
    store
        .unseal(
            Unlock::Recovery(secret(&second.to_lowercase())),
            "owner",
            NOW,
        )
        .expect("the new code opens the vault");
    assert_eq!(status_state(&store), "unsealed");
}

#[test]
fn a_key_file_with_0644_permissions_is_refused() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let (store, dir) = fresh("keyfile-mode");
        let key = dir.join("config").join("vault.key");
        assert_eq!(
            std::fs::metadata(&key).expect("meta").permissions().mode() & 0o777,
            0o600,
            "created 0600"
        );
        store.seal("owner", NOW).expect("sealed");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(matches!(
            store.unseal(Unlock::Keyfile, "owner", NOW),
            Err(VaultError::KeyFile(_))
        ));
        let reopened = Store::open(&dir, &key, None);
        assert!(
            matches!(reopened.boot(NOW), Err(VaultError::KeyFile(_))),
            "the hub does not start with it"
        );
    }
}

#[test]
fn rekey_keeps_every_value_and_changes_every_wrapped_key() {
    let (store, dir) = fresh("rekey");
    store
        .create(
            new(
                "ALPHA_KEY",
                "alpha-value-0000000000001",
                Scope::Plaza,
                &["hf"],
            ),
            "owner",
            NOW,
        )
        .expect("alpha");
    store
        .create(
            new(
                "BETA_KEY",
                "beta-value-00000000000002",
                nodes_scope(&["n0"]),
                &["hf"],
            ),
            "owner",
            NOW,
        )
        .expect("beta");
    let passphrase = store
        .add_passphrase(&secret("correct horse battery"), "owner", NOW)
        .expect("slot");
    let (slots_before, deks_before) = wrap_cts(&dir);
    let key_before = std::fs::read_to_string(dir.join("config").join("vault.key")).expect("key");
    store
        .rekey(
            &[(passphrase.clone(), secret("correct horse battery"))],
            "owner",
            NOW,
        )
        .expect("rekeyed");
    let (slots_after, deks_after) = wrap_cts(&dir);
    assert!(
        slots_before.iter().zip(&slots_after).all(|(a, b)| a != b),
        "every slot re-wrapped"
    );
    assert!(
        deks_before.iter().zip(&deks_after).all(|(a, b)| a != b),
        "every data key re-wrapped"
    );
    assert_ne!(
        key_before,
        std::fs::read_to_string(dir.join("config").join("vault.key")).expect("key"),
        "the key file is new"
    );
    let alpha = hub("hf");
    assert_eq!(
        store
            .resolve("ALPHA_KEY", &alpha, NOW)
            .expect("resolves")
            .expose_secret(),
        "alpha-value-0000000000001"
    );
    let reopened = reopen(&dir);
    // BETA_KEY is scoped to n0: the owner testing on the hub's machine resolves it.
    let beta = Consumer {
        binding: "hf".into(),
        on_behalf: OnBehalf::Owner { node: None },
    };
    assert_eq!(
        reopened
            .resolve("BETA_KEY", &beta, NOW)
            .expect("after a restart")
            .expose_secret(),
        "beta-value-00000000000002"
    );
}

#[test]
fn a_node_entry_beats_the_plaza_entry() {
    let (store, _dir) = fresh("node-beats-plaza");
    let name = "ANTHROPIC_API_KEY";
    store
        .create(
            new(
                name,
                "plaza-value-00000000001",
                Scope::Plaza,
                &["provider:anthropic"],
            ),
            "owner",
            NOW,
        )
        .expect("plaza");
    store
        .create(
            new(
                name,
                "node-one-value-0000002",
                nodes_scope(&["n1"]),
                &["provider:anthropic"],
            ),
            "owner",
            NOW,
        )
        .expect("node");
    let fleet = vec![node("n1", 1000, 100, false)];
    let on_n1 = Consumer::agent(&fleet, 1005, "provider:anthropic");
    assert_eq!(
        store
            .resolve(name, &on_n1, NOW)
            .expect("resolves")
            .expose_secret(),
        "node-one-value-0000002"
    );
    let on_n0 = Consumer::agent(&fleet, 210, "provider:anthropic");
    assert_eq!(
        store
            .resolve(name, &on_n0, NOW)
            .expect("resolves")
            .expose_secret(),
        "plaza-value-00000000001"
    );
}

#[test]
fn an_agent_on_another_node_is_out_of_scope() {
    let (store, _dir) = fresh("out-of-scope");
    store
        .create(
            new(
                "SLACK_TOKEN",
                "slack-value-000000000001",
                nodes_scope(&["n2"]),
                &["connector:slack"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    let fleet = vec![node("n1", 1000, 100, false), node("n2", 1100, 100, false)];
    let on_n1 = Consumer::agent(&fleet, 1005, "connector:slack");
    assert_eq!(
        store.resolve("SLACK_TOKEN", &on_n1, NOW).err(),
        Some(VaultError::OutOfScope)
    );
    let on_n2 = Consumer::agent(&fleet, 1105, "connector:slack");
    assert!(store.resolve("SLACK_TOKEN", &on_n2, NOW).is_ok());
}

#[test]
fn a_missing_binding_is_invisible() {
    let (store, _dir) = fresh("binding");
    store
        .create(
            new(
                "ANTHROPIC_API_KEY",
                "value-00000000000000003",
                Scope::Plaza,
                &["provider:anthropic"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    let other = Consumer {
        binding: "provider:openai".into(),
        on_behalf: OnBehalf::Hub,
    };
    assert_eq!(
        store.resolve("ANTHROPIC_API_KEY", &other, NOW).err(),
        Some(VaultError::NotFound)
    );
}

#[test]
fn the_node_comes_from_the_cid_range_not_the_request() {
    let fleet = vec![node("n1", 1000, 100, false), node("n2", 1100, 100, true)];
    assert_eq!(node_for_cid(&fleet, 1005).as_deref(), Some("n1"));
    assert_eq!(
        node_for_cid(&fleet, 210).as_deref(),
        Some("n0"),
        "a CID no joined node holds is the hub's machine"
    );
    assert_eq!(
        node_for_cid(&fleet, 1105),
        None,
        "a revoked node's CID fails closed"
    );
    let (store, _dir) = fresh("cid-range");
    store
        .create(
            new(
                "HF_TOKEN",
                "hf-value-0000000000001",
                nodes_scope(&["n1"]),
                &["hf"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    // The consumer is built from the CID only; a request cannot say another node.
    let consumer = Consumer::agent(&fleet, 1105, "hf");
    assert_eq!(
        store.resolve("HF_TOKEN", &consumer, NOW).err(),
        Some(VaultError::OutOfScope),
        "the revoked node gets nothing"
    );
    let on_n1 = Consumer::agent(&fleet, 1006, "hf");
    assert!(store.resolve("HF_TOKEN", &on_n1, NOW).is_ok());
}

#[test]
fn a_second_plaza_entry_and_overlapping_node_sets_are_409() {
    let (store, _dir) = fresh("conflicts");
    store
        .create(
            new(
                "OPENAI_API_KEY",
                "value-00000000000000004",
                Scope::Plaza,
                &["provider:openai"],
            ),
            "owner",
            NOW,
        )
        .expect("plaza");
    let twice = store.create(
        new(
            "OPENAI_API_KEY",
            "value-00000000000000005",
            Scope::Plaza,
            &["provider:openai"],
        ),
        "owner",
        NOW,
    );
    assert!(matches!(twice, Err(VaultError::Conflict(_))));
    store
        .create(
            new(
                "OPENAI_API_KEY",
                "value-00000000000000006",
                nodes_scope(&["n1"]),
                &["provider:openai"],
            ),
            "owner",
            NOW,
        )
        .expect("n1");
    let overlap = store.create(
        new(
            "OPENAI_API_KEY",
            "value-00000000000000007",
            nodes_scope(&["n1", "n2"]),
            &["provider:openai"],
        ),
        "owner",
        NOW,
    );
    assert!(matches!(overlap, Err(VaultError::Conflict(_))));
    store
        .create(
            new(
                "OPENAI_API_KEY",
                "value-00000000000000008",
                nodes_scope(&["n2"]),
                &["provider:openai"],
            ),
            "owner",
            NOW,
        )
        .expect("n2 is free");
}

#[test]
fn rotation_keeps_two_previous_versions_and_rollback_and_patch_keep_them_readable() {
    let (store, _dir) = fresh("versions");
    let id = store
        .create(
            new(
                "GITHUB_TOKEN",
                "version-one-000000001",
                Scope::Plaza,
                &["mcp:github"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    for n in 2..=4 {
        store
            .rotate(
                &id,
                &secret(&format!("version-{n}-000000000000001")),
                "owner",
                NOW,
            )
            .expect("rotated");
    }
    let view = store.view(&id).expect("view");
    assert_eq!(
        view["versions"].as_array().map(Vec::len),
        Some(3),
        "current and two before"
    );
    store.rollback(&id, 2, "owner", NOW).expect("rolled back");
    let consumer = hub("mcp:github");
    assert_eq!(
        store
            .resolve("GITHUB_TOKEN", &consumer, NOW)
            .expect("v2")
            .expose_secret(),
        "version-2-000000000000001"
    );
    store
        .patch(
            &id,
            None,
            Some(nodes_scope(&["n0"])),
            Some(vec!["mcp:github".into(), "hf".into()]),
            "owner",
            NOW,
        )
        .expect("patched");
    store
        .rollback(&id, 3, "owner", NOW)
        .expect("rolled back after the patch");
    let on_n0 = Consumer {
        binding: "mcp:github".into(),
        on_behalf: OnBehalf::Owner { node: None },
    };
    assert_eq!(
        store
            .resolve("GITHUB_TOKEN", &on_n0, NOW)
            .expect("v3 on n0")
            .expose_secret(),
        "version-3-000000000000001"
    );
    assert_eq!(
        store.resolve("GITHUB_TOKEN", &consumer, NOW).err(),
        Some(VaultError::OutOfScope),
        "the hub gets nothing from a nodes entry"
    );
}

#[test]
fn the_audit_chain_verifies_and_an_edited_line_breaks_it_at_that_line() {
    let (store, dir) = fresh("audit");
    store
        .create(
            new(
                "ALPHA_KEY",
                "alpha-value-0000000000009",
                Scope::Plaza,
                &["hf"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    store
        .create(
            new(
                "BETA_KEY",
                "beta-value-00000000000010",
                Scope::Plaza,
                &["hf"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    assert!(store.check_audit().expect("checked").ok);
    let path = dir.join("vault-audit.jsonl");
    let text = std::fs::read_to_string(&path).expect("read");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() >= 3);
    let edited: String = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 1 {
                line.replacen("\"owner\"", "\"ownex\"", 1)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{edited}\n")).expect("edited");
    let store = reopen(&dir);
    let verdict = store.check_audit().expect("checked");
    assert_eq!(
        (verdict.ok, verdict.broken_at),
        (false, Some(2)),
        "line 2 is where it breaks"
    );
    assert_eq!(verdict.verified, 1, "one entry verifies before it");
    let broken = verdict
        .first_broken
        .expect("the first broken entry is named");
    assert_eq!((broken.index, broken.seq), (1, Some(2)));
}

#[test]
fn a_deleted_entry_is_not_reimported_from_an_env_file() {
    let (store, _dir) = fresh("tombstone");
    let id = store
        .create(
            new("FOO_KEY", "foo-value-000000000001", Scope::Plaza, &["hf"]),
            "owner",
            NOW,
        )
        .expect("made");
    store.delete(&id, "owner", NOW).expect("deleted");
    let report = store
        .import_dotenv(
            "FOO_KEY=foo-value-000000000001\nNEW_KEY=new-value-00000000002\n",
            Scope::Plaza,
            &Default::default(),
            "owner",
            NOW,
        )
        .expect("imported");
    assert_eq!(
        report.skipped,
        vec![(
            "FOO_KEY".to_string(),
            "deleted before; not imported again".to_string()
        )]
    );
    assert_eq!(
        report
            .imported
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        vec!["NEW_KEY"]
    );
}

#[test]
fn import_skips_what_it_must_and_says_why() {
    let (store, _dir) = fresh("import-reasons");
    let text = "GOOD_KEY=good-value-000000000001\nlower=x\nEMPTY_KEY=\nDUP_KEY=1\nDUP_KEY=2\n";
    let report = store
        .import_dotenv(text, Scope::Plaza, &Default::default(), "owner", NOW)
        .expect("imported");
    assert_eq!(report.imported.len(), 1);
    let reasons: Vec<&str> = report.skipped.iter().map(|(_, r)| r.as_str()).collect();
    assert_eq!(
        reasons,
        vec![
            "invalid name",
            "empty value",
            "duplicate in the file",
            "duplicate in the file"
        ]
    );
}

#[test]
fn a_sealed_vault_answers_423_on_its_secret_routes() {
    let (store, _dir) = fresh("sealed-routes");
    store
        .create(
            new(
                "ALPHA_KEY",
                "alpha-value-0000000000011",
                Scope::Plaza,
                &["hf"],
            ),
            "owner",
            NOW,
        )
        .expect("made");
    store.seal("owner", NOW).expect("sealed");
    let bus = Bus::default();
    let nodes = nodes::Store::default();
    let ctx = Ctx {
        store: &store,
        bus: &bus,
        nodes: &nodes,
    };
    assert_eq!(routes::list(&ctx).status, 423);
    assert_eq!(
        routes::status(&ctx).status,
        200,
        "the status is always answered"
    );
}

/// Every route, every view, every event and the files on disk: the sentinel appears in none of them.
#[test]
fn no_value_appears_in_any_route_response_event_or_file() {
    let (store, dir) = fresh("no-leak");
    let bus = Bus::default();
    let events = bus.subscribe();
    let fleet = nodes::Store::default();
    let ctx = Ctx {
        store: &store,
        bus: &bus,
        nodes: &fleet,
    };
    let mut outputs: Vec<String> = Vec::new();
    let mut keep = |response: apz_http::Response| {
        outputs.push(String::from_utf8_lossy(response.bytes()).to_string())
    };

    let body = |value: Value| Request {
        body: value.to_string().into_bytes(),
        ..Default::default()
    };
    let empty = Request::default();

    keep(routes::status(&ctx));
    keep(routes::create(
        &ctx,
        &body(
            json!({ "name": "LEAK_KEY", "kind": "api_key", "label": "leak test", "value": SENTINEL, "scope": { "kind": "plaza" }, "bindings": ["provider:anthropic"] }),
        ),
    ));
    let id = store.list().expect("listed")[0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    keep(routes::list(&ctx));
    keep(Response::json(200, &store.view(&id).expect("view")));
    keep(routes::rotate(
        &ctx,
        &id,
        &body(json!({ "value": format!("{SENTINEL}-second") })),
    ));
    keep(routes::patch(
        &ctx,
        &id,
        &body(json!({ "label": "renamed", "bindings": ["provider:anthropic", "hf"] })),
    ));
    keep(routes::rollback(&ctx, &id, &body(json!({ "v": 1 }))));
    keep(routes::test(&ctx, &id, &empty, |_, _| {
        (true, String::new())
    }));
    keep(routes::import(
        &ctx,
        &body(
            json!({ "format": "dotenv", "text": format!("IMPORTED_KEY={SENTINEL}\n"), "scope": { "kind": "plaza" } }),
        ),
    ));
    keep(routes::import(
        &ctx,
        &body(
            json!({ "format": "dotenv", "text": format!("BAD=\"{SENTINEL}\n"), "scope": { "kind": "plaza" } }),
        ),
    ));
    keep(routes::consumers(&ctx));
    keep(routes::audit(
        &ctx,
        &Request {
            query: "verify=1".into(),
            ..Default::default()
        },
    ));
    keep(routes::audit(&ctx, &Request::default()));
    keep(routes::add_slot(
        &ctx,
        &body(json!({ "kind": "passphrase", "passphrase": format!("{SENTINEL}-pass") })),
    ));
    keep(routes::recovery(&ctx));
    keep(routes::seal(&ctx));
    let sealed = routes::list(&ctx);
    assert_eq!(sealed.status, 423);
    assert!(
        !String::from_utf8_lossy(sealed.bytes()).contains(HINT),
        "a sealed vault shows no hint"
    );
    keep(routes::unseal(
        &ctx,
        &body(json!({ "passphrase": format!("{SENTINEL}-wrong") })),
    ));
    keep(routes::unseal(
        &ctx,
        &body(json!({ "recovery_code": format!("{SENTINEL}-code") })),
    ));
    keep(routes::unseal(&ctx, &empty));
    keep(routes::rekey(&ctx, &empty));
    keep(routes::delete(&ctx, &id));

    let mut seen: Vec<String> = Vec::new();
    while let Ok(frame) = events.try_recv() {
        seen.push(frame.wire());
    }
    // Every file in the state directory, as bytes: the vault document and the audit chain.
    let mut files: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("state dir") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            files.push(String::from_utf8_lossy(&std::fs::read(&path).expect("read")).to_string());
        }
    }
    // The one place a hint may show in clear is an unsealed view's own `hint` field. The check strips
    // exactly that field from route responses, and nothing else: files and events are checked whole.
    let hint_fields = [
        format!("\"hint\":\"{HINT}\""),
        format!("\"hint\": \"{HINT}\""),
    ];
    assert!(
        outputs
            .iter()
            .any(|t| hint_fields.iter().any(|f| t.contains(f))),
        "an unsealed view shows the hint"
    );
    for (i, text) in outputs.iter().enumerate() {
        let mut text = text.clone();
        for field in &hint_fields {
            text = text.replace(field, "");
        }
        assert!(
            !text.contains(SENTINEL),
            "the sentinel leaked in route output {i}: {}",
            &text[..text.len().min(200)]
        );
        assert!(
            !text.contains(HINT),
            "the hint's characters leaked in route output {i}"
        );
    }
    for (i, text) in seen.iter().chain(files.iter()).enumerate() {
        assert!(
            !text.contains(SENTINEL),
            "the sentinel leaked in event or file {i}"
        );
        assert!(
            !text.contains(HINT),
            "the hint's characters leaked in event or file {i}"
        );
    }
    assert!(
        files
            .iter()
            .any(|t| t.contains("\"event\":\"create\"") || t.contains("\"event\": \"create\"")),
        "the audit did record the create"
    );
}

/// Writes every version's hint the way an older build kept it: in clear, under `hint`, re-signed.
fn write_legacy_hints(store: &Store) {
    let mut inner = store.lock();
    let keys = inner.keys.clone().expect("unsealed");
    let mut doc = inner.doc.clone().expect("a document");
    for entry in doc.entries.iter_mut() {
        for version in entry.versions.iter_mut() {
            if let Some(plain) = super::open_hint(&keys, &entry.id, version) {
                version.legacy_hint = Some(plain);
                version.hint_nonce = None;
                version.hint_ct = None;
            }
        }
    }
    super::sign(&keys, &mut doc);
    store.save(&doc).expect("saved");
    inner.doc = Some(doc);
}

#[test]
fn legacy_plaintext_hints_are_migrated_on_an_unsealed_start() {
    let (store, dir) = fresh("legacy-hint");
    let value = "legacy-hint-value-000Q7~!";
    let id = store
        .create(
            new("LEGACY_KEY", value, Scope::Plaza, &["hf"]),
            "owner",
            NOW,
        )
        .expect("made");
    write_legacy_hints(&store);
    let before: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("vault.json")).expect("read"))
            .expect("json");
    assert_eq!(
        before["entries"][0]["versions"][0]["hint"], HINT,
        "the old file holds the hint in clear"
    );
    assert!(before["entries"][0]["versions"][0].get("hint_ct").is_none());

    store.seal("owner", NOW).expect("sealed");
    assert!(store.list().is_err(), "a sealed vault shows no hints");
    assert!(
        !store.status(&[]).to_string().contains(HINT),
        "the status never shows a hint"
    );
    drop(store);

    // The unsealed start (the key file) migrates the file.
    let store = reopen(&dir);
    assert_eq!(status_state(&store), "unsealed");
    let after: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("vault.json")).expect("read"))
            .expect("json");
    let version = &after["entries"][0]["versions"][0];
    assert!(
        version.get("hint").is_none(),
        "no plaintext hint is left in the file"
    );
    assert!(
        version["hint_nonce"].is_string() && version["hint_ct"].is_string(),
        "the hint is sealed"
    );
    assert_eq!(
        store.view(&id).expect("view")["versions"][0]["hint"],
        HINT,
        "the view shows the same hint"
    );
    assert_eq!(
        store
            .resolve("LEGACY_KEY", &hub("hf"), NOW)
            .expect("resolves")
            .expose_secret(),
        value,
        "the value is unchanged"
    );

    // Once is enough: a second unsealed start leaves the file byte for byte as the seal saved it. (The
    // seal writes the usage counters the resolve above moved, so the snapshot comes after it.)
    store.seal("owner", NOW).expect("sealed");
    let migrated = std::fs::read(dir.join("vault.json")).expect("read");
    drop(store);
    let store = reopen(&dir);
    assert_eq!(
        std::fs::read(dir.join("vault.json")).expect("read"),
        migrated,
        "the second start changes nothing"
    );
    let audit = std::fs::read_to_string(dir.join("vault-audit.jsonl")).expect("read");
    let migrates: Vec<Value> = audit
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("a line"))
        .filter(|r| r["event"] == "migrate")
        .collect();
    assert_eq!(
        migrates.len(),
        1,
        "one migrate line for the whole migration"
    );
    assert_eq!(migrates[0]["detail"], "hints encrypted");
    assert_eq!(store.view(&id).expect("view")["versions"][0]["hint"], HINT);
}

#[test]
fn a_hint_is_bound_to_its_entry_and_version() {
    let (store, _dir) = fresh("hint-aad");
    let keys = store.lock().keys.clone().expect("unsealed");
    let (nonce, ct) = super::seal_hint(&keys, "sec_a", 1, HINT).expect("sealed");
    let nonce = super::model::dec(&nonce).expect("nonce");
    let ct = super::model::dec(&ct).expect("ciphertext");
    let plain = super::crypto::open(&keys.index, &nonce, &super::hint_aad("sec_a", 1), &ct)
        .expect("opens for its own entry and version");
    assert_eq!(plain.as_slice(), HINT.as_bytes());
    assert!(
        super::crypto::open(&keys.index, &nonce, &super::hint_aad("sec_a", 2), &ct).is_err(),
        "not for another version"
    );
    assert!(
        super::crypto::open(&keys.index, &nonce, &super::hint_aad("sec_b", 1), &ct).is_err(),
        "not for another entry"
    );
}

fn unsealed_with(store: &Store) -> Vec<(String, bool)> {
    store.status(&[])["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .map(|s| {
            (
                s["id"].as_str().unwrap_or_default().to_string(),
                s["unsealed_with"]
                    .as_bool()
                    .expect("unsealed_with is a bool"),
            )
        })
        .collect()
}

#[test]
fn status_marks_only_the_slot_that_unsealed_the_vault() {
    let (store, _dir) = fresh("unsealed-with");
    let keyfile = keyfile_slot(&store);
    let passphrase = store
        .add_passphrase(&secret("correct horse battery"), "owner", NOW)
        .expect("added");
    // Booted from the key file: only the key-file slot is marked.
    let marks = unsealed_with(&store);
    assert!(
        marks.contains(&(keyfile.clone(), true)),
        "the key-file slot unsealed the vault"
    );
    assert!(
        marks.contains(&(passphrase.clone(), false)),
        "the passphrase slot did not"
    );

    // Sealed: nothing is marked.
    store.seal("owner", NOW).expect("sealed");
    assert!(
        unsealed_with(&store).iter().all(|(_, marked)| !marked),
        "a sealed vault marks no slot"
    );

    // Unsealed with the passphrase: the mark moves to it.
    store
        .unseal(
            Unlock::Passphrase(secret("correct horse battery")),
            "owner",
            NOW,
        )
        .expect("unsealed");
    let marks = unsealed_with(&store);
    assert!(
        marks.contains(&(passphrase, true)),
        "the passphrase slot unsealed the vault this time"
    );
    assert!(
        marks.contains(&(keyfile, false)),
        "the key-file slot is no longer the one"
    );
}

/// The egress credential rules name their secret by the `egress` binding, so the vault takes it;
/// other bare words are still refused.
#[test]
fn the_egress_binding_is_a_binding() {
    assert!(super::valid_binding("egress"));
    assert!(super::valid_binding("hf"));
    assert!(super::valid_binding("provider:openai"));
    assert!(!super::valid_binding("egres"));
    assert!(!super::valid_binding("egress:x"));
    assert!(super::valid_binding("hub:weft"));
    assert!(super::valid_binding("hub:git-platforms"));
    assert!(!super::valid_binding("hub:"));
    // A usage hub's management key (usage_providers).
    assert!(super::valid_binding("usage:home"));
    assert!(!super::valid_binding("usage:"));
}
