//! The `SkillStore` contract, run against the SQLite store (`DbSkills`), and the `Skills` rules on top (preparing
//! versions into blobs, publishing with errors refused).

use std::sync::Arc;

use super::blobs::StoreBlobs;
use super::*;

const T0: u64 = 1_800_000_000_000;

const GOOD: &str = "---\nname: pdf-forms\ndescription: Fill PDF forms. Use when the task involves a PDF with form fields.\n---\n# PDF forms\n\nRead [the reference](reference.md) first.\n";

/// Blobs in memory, for the tests.
#[derive(Default)]
pub(crate) struct MemBlobs(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl Blobs for MemBlobs {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let hash = apz_blobs::Hash::of(bytes).to_string();
        self.0.lock().unwrap().insert(hash.clone(), bytes.to_vec());
        Ok(hash)
    }
    fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
        self.0
            .lock()
            .unwrap()
            .get(hash)
            .cloned()
            .ok_or_else(|| format!("no blob {hash}"))
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
    let skills = skills_on(Arc::new(store_db::DbSkills::for_tests()));
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
fn store_keeps_the_contract() {
    conformance(Arc::new(store_db::DbSkills::for_tests()));
}

#[test]
fn skills_survive_a_reopen() {
    let dir = tempfile::Builder::new()
        .prefix("apid-skills-reopen")
        .tempdir()
        .expect("a temporary folder");
    let blobs: Arc<dyn Blobs> = Arc::new(MemBlobs::default());
    {
        let store = crate::store::open(dir.path()).expect("the store opens");
        let skills = Skills::new(
            Arc::new(store_db::DbSkills::new(Arc::clone(
                store.db(crate::store::PLAZA),
            ))),
            Arc::clone(&blobs),
        );
        skills
            .store()
            .create(
                &new_skill("pdf", &["role:builder"]),
                &version(&skills, GOOD, "first"),
                "owner",
                T0,
            )
            .expect("creates");
        skills
            .store()
            .add_version(
                "pdf",
                1,
                &version(&skills, &format!("{GOOD}\nMore.\n"), "two"),
                "owner",
                T0 + 1,
            )
            .expect("adds a version");
        skills
            .store()
            .set_active("pdf", Some(2), "owner", T0 + 2)
            .expect("publishes");
        skills
            .store()
            .set_meta("pdf", "Kept title", &["docs".into()], "owner", T0 + 3)
            .expect("edits");
    }

    let store = crate::store::open(dir.path()).expect("the store opens again");
    let skills = Skills::new(
        Arc::new(store_db::DbSkills::new(Arc::clone(
            store.db(crate::store::PLAZA),
        ))),
        blobs,
    );
    let back = skills
        .store()
        .get("pdf")
        .expect("reads")
        .expect("the skill is there");
    assert_eq!(back.title, "Kept title");
    assert_eq!(back.tags, vec!["docs".to_string()]);
    assert_eq!(back.bindings, vec!["role:builder".to_string()]);
    assert_eq!((back.latest_version, back.active_version), (2, Some(2)));
    let v1 = skills
        .store()
        .version("pdf", 1)
        .expect("reads")
        .expect("version 1 is there");
    assert_eq!(v1.skill_md, GOOD);
    assert_eq!((v1.files.len(), v1.meta.file_count), (1, 1));
    assert_eq!(skills.store().versions("pdf").expect("lists").len(), 2);
    assert_eq!(
        skills.store().bound("role:builder").expect("binds").len(),
        1
    );
    let actions: Vec<Action> = skills
        .store()
        .events("pdf", 50)
        .expect("reads the history")
        .iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(
        actions,
        vec![
            Action::Edit,
            Action::Publish,
            Action::Version,
            Action::Version,
            Action::Create
        ]
    );
}

#[test]
fn prepare_refuses_unsafe_paths_and_publish_refuses_errors() {
    let skills = skills_on(Arc::new(store_db::DbSkills::for_tests()));
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

/// A scratch folder for one test's object store: under the temp dir, named with this process's id and the test's
/// `name`, and emptied first. The tests never touch the real state dir.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("apid-skills-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn file_in(path: &str, bytes: &[u8]) -> FileIn {
    FileIn {
        path: path.into(),
        body: FileBody::Bytes(bytes.to_vec()),
        executable: false,
    }
}

#[test]
fn a_version_file_is_kept_in_the_object_store_under_its_hash() {
    let dir = scratch("version-file");
    let objects = Arc::new(apz_blobs::Blobs::open(&dir).expect("opens the object store"));
    let skills = Skills::new(
        Arc::new(store_db::DbSkills::for_tests()),
        Arc::new(StoreBlobs::new(Arc::clone(&objects))),
    );
    let reference = b"# Reference\n";
    let first = skills
        .prepare(
            GOOD.into(),
            vec![file_in("reference.md", reference)],
            String::new(),
            None,
        )
        .expect("prepares");
    let hash = apz_blobs::Hash::of(reference).to_string();
    assert_eq!(first.files[0].hash, hash);
    assert_eq!(first.hash, apz_blobs::Hash::of(GOOD.as_bytes()).to_string());
    skills
        .store()
        .create(&new_skill("pdf", &[]), &first, "owner", T0)
        .expect("creates");
    let stored = skills.store().version("pdf", 1).unwrap().unwrap();
    assert_eq!(stored.files[0].hash, hash);
    assert_eq!(stored.meta.hash, first.hash);
    assert_eq!(skills.blobs().get(&hash).unwrap(), reference.to_vec());
    assert!(objects.has(&apz_blobs::Hash::of(reference)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_blobs_read_only_lower_case_hashes_the_store_holds() {
    let dir = scratch("store-blobs");
    let objects = Arc::new(apz_blobs::Blobs::open(&dir).expect("opens the object store"));
    let blobs = StoreBlobs::new(objects);
    let hash = blobs.put(b"reference").expect("puts");
    assert_eq!(hash, apz_blobs::Hash::of(b"reference").to_string());
    assert_eq!(
        blobs.put(b"reference").expect("puts again"),
        hash,
        "put is idempotent"
    );
    assert_eq!(blobs.get(&hash).expect("reads"), b"reference".to_vec());
    assert!(
        blobs.get(&hash.to_uppercase()).is_err(),
        "upper-case hex is not the stored text"
    );
    assert!(blobs.get("abc").is_err(), "a short text is not a hash");
    assert!(
        blobs.get(&"0".repeat(64)).is_err(),
        "a hash the store does not hold"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hashes_are_every_file_of_every_version_archived_or_not() {
    let skills = skills_on(Arc::new(store_db::DbSkills::for_tests()));
    assert_eq!(skills.hashes(), Ok(Vec::new()));
    let first = skills
        .prepare(
            GOOD.into(),
            vec![file_in("reference.md", b"one")],
            String::new(),
            None,
        )
        .expect("prepares");
    skills
        .store()
        .create(&new_skill("pdf", &[]), &first, "owner", T0)
        .expect("creates");
    let second = skills
        .prepare(
            format!("{GOOD}\nMore.\n"),
            vec![file_in("reference.md", b"two")],
            "two".into(),
            None,
        )
        .expect("prepares");
    skills
        .store()
        .add_version("pdf", 1, &second, "owner", T0 + 1)
        .expect("adds a version");
    skills
        .store()
        .set_archived("pdf", true, "owner", T0 + 2)
        .expect("archives");
    // A live skill that holds the same bytes as the archived skill's first version: listed once.
    let live = skills
        .prepare(
            GOOD.into(),
            vec![file_in("reference.md", b"one")],
            String::new(),
            None,
        )
        .expect("prepares");
    skills
        .store()
        .create(&new_skill("pdf2", &[]), &live, "owner", T0 + 3)
        .expect("creates");
    let mut want = vec![apz_blobs::Hash::of(b"one"), apz_blobs::Hash::of(b"two")];
    want.sort();
    assert_eq!(skills.hashes(), Ok(want));
}

#[test]
fn a_known_hash_reads_its_bytes_back_from_the_store() {
    let dir = scratch("known-hash");
    let objects = Arc::new(apz_blobs::Blobs::open(&dir).expect("opens the object store"));
    let skills = Skills::new(
        Arc::new(store_db::DbSkills::for_tests()),
        Arc::new(StoreBlobs::new(objects)),
    );
    let reference = b"# Reference\n";
    let first = skills
        .prepare(
            GOOD.into(),
            vec![file_in("reference.md", reference)],
            String::new(),
            None,
        )
        .expect("prepares");
    skills
        .store()
        .create(&new_skill("pdf", &[]), &first, "owner", T0)
        .expect("creates");
    // A new version keeps the file by its hash alone: prepare reads the bytes back from the store.
    let hash = first.files[0].hash.clone();
    let kept = FileIn {
        path: "reference.md".into(),
        body: FileBody::Known(hash.clone()),
        executable: false,
    };
    let next = skills
        .prepare(format!("{GOOD}\nMore.\n"), vec![kept], "kept".into(), None)
        .expect("prepares the kept file");
    assert_eq!(
        next.files,
        vec![SkillFile {
            path: "reference.md".into(),
            hash,
            size: reference.len() as u64,
            executable: false,
        }]
    );
    // A hash the store does not hold, or text that is not a hash, is refused.
    for unknown in ["0".repeat(64), "not-a-hash".to_string()] {
        let refused = FileIn {
            path: "reference.md".into(),
            body: FileBody::Known(unknown),
            executable: false,
        };
        assert!(matches!(
            skills.prepare(GOOD.into(), vec![refused], String::new(), None),
            Err(StoreError::Invalid(_))
        ));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
