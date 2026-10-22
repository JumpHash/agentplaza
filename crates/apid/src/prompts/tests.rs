//! The `PromptStore` contract, run against `SqlitePrompts` in a temporary `plaza.db`, and the reopen of that store.
//! Plus the `Prompts` rules on top (unchanged bodies, publishing with errors, the cache).

use std::sync::Arc;

use apz_prompt::Kind;

use super::store::SqlitePrompts;
use super::*;

const T0: u64 = 1_800_000_000_000;

const GOOD_SYSTEM: &str = "# Reviewer\n\n## Purpose\nYou review code.\n\n## Responsibilities\n- Read each change.\n\n\
                           ## Deliverables\n- A review.\n\n## Boundaries\n- Review only what you were given.\n";

fn new_prompt(id: &str, kind: Kind, binding: Option<&str>) -> NewPrompt {
    NewPrompt {
        id: id.into(),
        kind,
        title: format!("Title of {id}"),
        description: String::new(),
        tags: vec!["t".into()],
        binding: binding.map(str::to_string),
    }
}

fn version(kind: Kind, body: &str) -> NewVersion {
    NewVersion::new(kind, body.to_string(), "note".into())
}

/// The contract every store keeps.
pub(crate) fn conformance(store: &dyn PromptStore) {
    // create: version 1, unpublished, events Create then Version.
    let p = store
        .create(
            &new_prompt("rev", Kind::System, Some("role:reviewer")),
            &version(Kind::System, GOOD_SYSTEM),
            "owner",
            T0,
        )
        .unwrap();
    assert_eq!(
        (p.latest_version, p.active_version, p.archived_ms),
        (1, None, None)
    );
    assert_eq!(p.binding.as_deref(), Some("role:reviewer"));
    assert!(matches!(
        store.create(
            &new_prompt("rev", Kind::System, None),
            &version(Kind::System, "x"),
            "owner",
            T0
        ),
        Err(StoreError::Conflict(_))
    ));
    // A second live prompt cannot take the same binding.
    assert!(matches!(
        store.create(
            &new_prompt("rev2", Kind::System, Some("role:reviewer")),
            &version(Kind::System, "x"),
            "owner",
            T0
        ),
        Err(StoreError::Conflict(_))
    ));

    // add_version: base must be the latest.
    let v2 = store
        .add_version(
            "rev",
            1,
            &version(Kind::System, &format!("{GOOD_SYSTEM}\nMore.\n")),
            "owner",
            T0 + 1,
        )
        .unwrap();
    assert_eq!(v2.version, 2);
    assert!(matches!(
        store.add_version("rev", 1, &version(Kind::System, "y"), "owner", T0 + 2),
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(
        store.add_version("nope", 1, &version(Kind::System, "y"), "owner", T0),
        Err(StoreError::NotFound)
    );
    let metas = store.versions("rev").unwrap();
    assert_eq!(
        metas.iter().map(|m| m.version).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(metas[0].sha256, sha256_hex(GOOD_SYSTEM));
    let full = store.version("rev", 1).unwrap().unwrap();
    assert_eq!(full.body, GOOD_SYSTEM);
    assert_eq!(
        full.findings,
        apz_prompt::check(Kind::System, GOOD_SYSTEM).findings
    );
    assert!(store.version("rev", 9).unwrap().is_none());

    // bound: only once published.
    assert!(store.bound("role:reviewer").unwrap().is_none());
    let p = store.set_active("rev", Some(1), "owner", T0 + 3).unwrap();
    assert_eq!(p.active_version, Some(1));
    assert_eq!(
        store.bound("role:reviewer").unwrap().unwrap().1,
        GOOD_SYSTEM
    );
    assert!(matches!(
        store.set_active("rev", Some(7), "owner", T0),
        Err(StoreError::Invalid(_))
    ));

    // archive releases the binding; restore conflicts when it was taken meanwhile.
    store.set_archived("rev", true, "owner", T0 + 4).unwrap();
    assert!(store.bound("role:reviewer").unwrap().is_none());
    assert!(store.list(false).unwrap().iter().all(|p| p.id != "rev"));
    assert!(store.list(true).unwrap().iter().any(|p| p.id == "rev"));
    store
        .create(
            &new_prompt("rev2", Kind::System, Some("role:reviewer")),
            &version(Kind::System, GOOD_SYSTEM),
            "owner",
            T0 + 5,
        )
        .unwrap();
    assert!(matches!(
        store.set_archived("rev", false, "owner", T0 + 6),
        Err(StoreError::Conflict(_))
    ));

    // set_meta: the binding is checked against other live prompts.
    let meta = Meta {
        title: "New".into(),
        description: "d".into(),
        tags: vec![],
        binding: Some("role:reviewer".into()),
    };
    store
        .create(
            &new_prompt("other", Kind::System, None),
            &version(Kind::System, GOOD_SYSTEM),
            "owner",
            T0 + 7,
        )
        .unwrap();
    assert!(matches!(
        store.set_meta("other", &meta, "owner", T0 + 8),
        Err(StoreError::Conflict(_))
    ));
    let meta = Meta {
        binding: None,
        ..meta
    };
    assert_eq!(
        store
            .set_meta("other", &meta, "owner", T0 + 8)
            .unwrap()
            .title,
        "New"
    );

    // unpublish.
    store.set_active("rev2", Some(1), "owner", T0 + 9).unwrap();
    assert!(store.bound("role:reviewer").unwrap().is_some());
    assert_eq!(
        store
            .set_active("rev2", None, "owner", T0 + 10)
            .unwrap()
            .active_version,
        None
    );
    assert!(store.bound("role:reviewer").unwrap().is_none());

    // history, newest first.
    let events = store.events("rev", 50).unwrap();
    let actions: Vec<_> = events.iter().map(|e| e.action).collect();
    assert_eq!(
        actions,
        vec![
            Action::Archive,
            Action::Publish,
            Action::Version,
            Action::Version,
            Action::Create
        ]
    );
    assert_eq!(events[1].version, Some(1));
    assert_eq!(events[0].actor, "owner");
    assert_eq!(store.events("rev", 2).unwrap().len(), 2);

    // list is by id.
    let ids: Vec<_> = store
        .list(true)
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec!["other", "rev", "rev2"]);
}

#[test]
fn sqlite_store_keeps_the_contract() {
    conformance(&SqlitePrompts::for_tests());
}

#[test]
fn sqlite_store_survives_a_reopen() {
    let dir = tempfile::Builder::new()
        .prefix("apid-prompts-reopen")
        .tempdir()
        .expect("a temporary folder");
    let body = format!("{GOOD_SYSTEM}\nMore.\n");
    {
        let store = crate::store::open_for_tests(dir.path()).expect("the store opens");
        let prompts = SqlitePrompts::new(Arc::clone(store.db(crate::store::PLAZA)));
        prompts
            .create(
                &new_prompt("rev", Kind::System, Some("role:reviewer")),
                &version(Kind::System, GOOD_SYSTEM),
                "owner",
                T0,
            )
            .unwrap();
        prompts
            .add_version("rev", 1, &version(Kind::System, &body), "owner", T0 + 1)
            .unwrap();
        prompts.set_active("rev", Some(2), "owner", T0 + 2).unwrap();
    }
    let store = crate::store::open_for_tests(dir.path()).expect("the store opens again");
    let prompts = SqlitePrompts::new(Arc::clone(store.db(crate::store::PLAZA)));
    let rev = prompts.get("rev").unwrap().unwrap();
    assert_eq!(rev.tags, vec!["t".to_string()]);
    assert_eq!(rev.binding.as_deref(), Some("role:reviewer"));
    assert_eq!((rev.latest_version, rev.active_version), (2, Some(2)));
    assert_eq!(prompts.bound("role:reviewer").unwrap().unwrap().1, body);
    let first = prompts.version("rev", 1).unwrap().unwrap();
    assert_eq!(first.body, GOOD_SYSTEM);
    assert_eq!(
        first.findings,
        apz_prompt::check(Kind::System, GOOD_SYSTEM).findings
    );
    // The history comes back whole, and the next event takes the id after the last one written before the reopen.
    prompts.set_active("rev", None, "owner", T0 + 3).unwrap();
    let events = prompts.events("rev", 50).unwrap();
    assert_eq!(
        events.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![5, 4, 3, 2, 1]
    );
    assert_eq!(events[0].action, Action::Unpublish);
}

#[test]
fn prompts_refuses_to_publish_a_version_with_errors_and_skips_unchanged_bodies() {
    let prompts = Prompts::new(Arc::new(SqlitePrompts::for_tests()));
    let bad = "# Bad\n\n```\nunclosed fence\n";
    prompts
        .create(
            &new_prompt("bad", Kind::Fragment, None),
            bad.into(),
            String::new(),
            "owner",
            T0,
        )
        .unwrap();
    assert!(matches!(
        prompts.publish("bad", 1, "owner", T0),
        Err(StoreError::Invalid(_))
    ));
    let (meta, created) = prompts
        .add_version("bad", 1, bad.into(), "same".into(), "owner", T0)
        .unwrap();
    assert_eq!((meta.version, created), (1, false));
}

#[test]
fn bound_body_is_cached_and_cleared_by_writes() {
    let prompts = Prompts::new(Arc::new(SqlitePrompts::for_tests()));
    assert_eq!(prompts.bound_body("plaza"), None);
    let rules = "# House rules\n\n## Setup\n- `cargo build`\n\n## Testing\n- `cargo test`\n";
    prompts
        .create(
            &new_prompt("house", Kind::AgentsMd, Some("plaza")),
            rules.into(),
            String::new(),
            "owner",
            T0,
        )
        .unwrap();
    prompts.publish("house", 1, "owner", T0 + 1).unwrap();
    assert_eq!(prompts.bound_body("plaza").as_deref(), Some(rules));
}

#[test]
fn bindings_follow_the_kind() {
    assert!(valid_binding(Kind::System, "role:builder"));
    assert!(!valid_binding(Kind::System, "plaza"));
    assert!(valid_binding(Kind::AgentsMd, "plaza"));
    assert!(!valid_binding(Kind::AgentsMd, "role:builder"));
    assert!(!valid_binding(Kind::Fragment, "plaza"));
    assert!(!valid_binding(Kind::System, "role:Builder"));
    assert!(valid_id("role-builder") && !valid_id("-x") && !valid_id("A") && !valid_id(""));
}
