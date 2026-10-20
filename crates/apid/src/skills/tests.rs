//! The `SkillStore` contract, run against both stores (`MemorySkills` always, `DbSkills` with APZ_TEST_DATABASE_URL),
//! and the `Skills` rules on top (preparing versions into blobs, publishing with errors refused).

use std::sync::Arc;

use super::*;

const T0: u64 = 1_800_000_000_000;

const GOOD: &str = "---\nname: pdf-forms\ndescription: Fill PDF forms. Use when the task involves a PDF with form fields.\n---\n# PDF forms\n\nRead [the reference](reference.md) first.\n";

/// Blobs in memory, for the tests.
#[derive(Default)]
pub(crate) struct MemBlobs(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl Blobs for MemBlobs {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let sha = sha256_hex(bytes);
        self.0.lock().unwrap().insert(sha.clone(), bytes.to_vec());
        Ok(sha)
    }
    fn get(&self, sha256: &str) -> Result<Vec<u8>, String> {
        self.0
            .lock()
            .unwrap()
            .get(sha256)
            .cloned()
            .ok_or_else(|| format!("no blob {sha256}"))
    }
}

fn skills_on(store: Arc<dyn SkillStore>) -> Skills {
    Skills::new(store, Arc::new(MemBlobs::default()))
}

fn version(skills: &Skills, skill_md: &str, note: &str) -> NewVersion {
    let files = vec![FileIn {
        path: "reference.md".into(),
        body: FileBody::Bytes(b"# Reference\n".to_vec()),
        executable: false,
    }];
    skills
        .prepare(skill_md.to_string(), files, note.to_string(), None)
        .expect("prepares")
}

fn new_skill(id: &str, bindings: &[&str]) -> NewSkill {
    NewSkill {
        id: id.into(),
        title: format!("Title of {id}"),
        tags: vec![],
        bindings: bindings.iter().map(|b| b.to_string()).collect(),
        source: Source::Built,
    }
}

/// The contract every store keeps.
pub(crate) fn conformance(store: Arc<dyn SkillStore>) {
    let skills = skills_on(Arc::clone(&store));
    let s = store
        .create(
            &new_skill("pdf", &["role:builder"]),
            &version(&skills, GOOD, "first"),
            "owner",
            T0,
        )
        .unwrap();
    assert_eq!(
        (s.latest_version, s.active_version, s.name.as_str()),
        (1, None, "pdf-forms")
    );
    assert_eq!(s.bindings, vec!["role:builder".to_string()]);
    // A second live skill may not take the same name, nor the same id.
    assert!(matches!(
        store.create(
            &new_skill("pdf2", &[]),
            &version(&skills, GOOD, ""),
            "owner",
            T0
        ),
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.create(
            &new_skill("pdf", &[]),
            &version(&skills, GOOD, ""),
            "owner",
            T0
        ),
        Err(StoreError::Conflict(_))
    ));

    // Versions: base must be the latest; files are kept.
    let v2 = store
        .add_version(
            "pdf",
            1,
            &version(&skills, &format!("{GOOD}\nMore.\n"), "two"),
            "owner",
            T0 + 1,
        )
        .unwrap();
    assert_eq!(v2.version, 2);
    assert!(matches!(
        store.add_version("pdf", 1, &version(&skills, GOOD, ""), "owner", T0 + 2),
        Err(StoreError::Conflict(_))
    ));
    let full = store.version("pdf", 1).unwrap().unwrap();
    assert_eq!(full.skill_md, GOOD);
    assert_eq!(full.files.len(), 1);
    assert_eq!(full.files[0].path, "reference.md");
    assert_eq!(full.meta.file_count, 1);

    // Bound only once published; bindings replace.
    assert!(store.bound("role:builder").unwrap().is_empty());
    store.set_active("pdf", Some(2), "owner", T0 + 3).unwrap();
    let bound = store.bound("role:builder").unwrap();
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].1.version, 2);
    store
        .set_bindings("pdf", &["plaza".to_string()], "owner", T0 + 4)
        .unwrap();
    assert!(store.bound("role:builder").unwrap().is_empty());
    assert_eq!(store.bound("plaza").unwrap().len(), 1);
    assert!(matches!(
        store.set_active("pdf", Some(9), "owner", T0),
        Err(StoreError::Invalid(_))
    ));

    // Archive frees the name; restore conflicts once it is taken.
    store.set_archived("pdf", true, "owner", T0 + 5).unwrap();
    assert!(store.bound("plaza").unwrap().is_empty());
    store
        .create(
            &new_skill("pdf2", &[]),
            &version(&skills, GOOD, ""),
            "owner",
            T0 + 6,
        )
        .unwrap();
    assert!(matches!(
        store.set_archived("pdf", false, "owner", T0 + 7),
        Err(StoreError::Conflict(_))
    ));

    // Meta and history.
    assert_eq!(
        store
            .set_meta("pdf2", "New title", &["docs".into()], "owner", T0 + 8)
            .unwrap()
            .title,
        "New title"
    );
    let actions: Vec<Action> = store
        .events("pdf", 50)
        .unwrap()
        .iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(
        actions,
        vec![
            Action::Archive,
            Action::Bind,
            Action::Publish,
            Action::Version,
            Action::Version,
            Action::Create
        ]
    );
    let ids: Vec<String> = store
        .list(true)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, vec!["pdf", "pdf2"]);
}

#[test]
fn builtin_skills_are_seeded_once() {
    let skills = skills_on(Arc::new(store_mem::MemorySkills::in_memory()));
    assert_eq!(seed_builtins(&skills, "owner", T0), Ok(1));
    assert_eq!(seed_builtins(&skills, "owner", T0 + 1), Ok(0));
    let held: Vec<Skill> = skills
        .store()
        .list(false)
        .expect("lists")
        .into_iter()
        .filter(|skill| skill.id == "show-your-work")
        .collect();
    assert_eq!(held.len(), 1, "seeding twice holds the skill once");
    assert_eq!(
        held[0].active_version,
        Some(1),
        "the built-in skill is published"
    );
    assert_eq!(held[0].source, Source::Built);
    skills
        .store()
        .set_archived("show-your-work", true, "owner", T0 + 2)
        .expect("archives");
    assert_eq!(
        seed_builtins(&skills, "owner", T0 + 3),
        Ok(0),
        "an archived built-in skill is not seeded again"
    );
}

#[test]
fn memory_store_keeps_the_contract() {
    conformance(Arc::new(store_mem::MemorySkills::in_memory()));
}

#[test]
fn db_store_keeps_the_contract() {
    let Some(url) = crate::db::test_database("skills_contract") else {
        return;
    };
    let env = std::collections::BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
    let db = Arc::new(crate::db::Db::connect(&env));
    assert!(db.enabled());
    conformance(Arc::new(store_db::DbSkills::new(db)));
}

#[test]
fn prepare_refuses_unsafe_paths_and_publish_refuses_errors() {
    let skills = skills_on(Arc::new(store_mem::MemorySkills::in_memory()));
    let bad_path = vec![FileIn {
        path: "../x".into(),
        body: FileBody::Bytes(vec![1]),
        executable: false,
    }];
    assert!(matches!(
        skills.prepare(GOOD.into(), bad_path, String::new(), None),
        Err(StoreError::Invalid(_))
    ));
    let broken = "no frontmatter at all\n";
    let v = skills
        .prepare(broken.into(), vec![], String::new(), None)
        .unwrap();
    skills
        .store()
        .create(&new_skill("broken", &[]), &v, "owner", T0)
        .unwrap();
    assert!(matches!(
        skills.publish("broken", 1, "owner", T0),
        Err(StoreError::Invalid(_))
    ));
}

#[test]
fn paths_and_bindings() {
    assert!(
        safe_path("scripts/fill.py")
            && !safe_path("a/../b")
            && !safe_path("/etc")
            && !safe_path("SKILL.md")
            && !safe_path("a//b")
    );
    assert!(
        valid_binding("plaza")
            && valid_binding("role:builder")
            && !valid_binding("role:")
            && !valid_binding("everyone")
    );
}
