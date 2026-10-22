//! derive(Table) and derive(Column): structs round-trip through typed tables, saves write only what changed, and the
//! generated schema passes the lint.

use std::collections::{BTreeMap, BTreeSet};

use apz_store::rusqlite::Connection;
use apz_store::table::{self, load_list, load_one};
use apz_store::{Column, RowSet, Saved, Table, lint};

#[derive(Debug, Clone, PartialEq, Eq, Column)]
#[column(rename_all = "snake_case")]
enum Stage {
    Draft,
    InReview,
    #[table(rename = "done")]
    Finished,
}

#[derive(Debug, Clone, PartialEq, Column)]
struct Cid(u64);

#[derive(Debug, Clone, PartialEq, Table)]
enum Auth {
    None,
    Key { env: String, header: Option<String> },
    Command(String),
}

#[derive(Debug, Clone, PartialEq, Default, Table)]
struct Limits {
    turns: u32,
    seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Table)]
struct Note {
    at_ms: u64,
    text: String,
    #[table(list)]
    tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Table)]
struct Doc {
    #[table(key)]
    id: String,
    title: String,
    #[table(child)]
    notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "project")]
struct Project {
    #[table(key)]
    id: String,
    title: String,
    stage: Stage,
    lead: Cid,
    paused: bool,
    budget: Option<f64>,
    #[table(flatten)]
    limits: Limits,
    #[table(flatten)]
    auth: Auth,
    #[table(flatten)]
    extra: Option<Limits>,
    #[table(child)]
    docs: Vec<Doc>,
    #[table(child)]
    cover: Option<Note>,
    #[table(list)]
    tasks: Vec<String>,
    #[table(set)]
    members: BTreeSet<u64>,
    #[table(map)]
    by_name: BTreeMap<String, Note>,
    #[table(map_value)]
    labels: BTreeMap<String, i64>,
    #[table(content)]
    prompt: String,
    #[table(skip)]
    cache: Vec<u8>,
}

fn note(at: u64, text: &str, tags: &[&str]) -> Note {
    Note {
        at_ms: at,
        text: text.into(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

fn project(id: &str) -> Project {
    Project {
        id: id.into(),
        title: format!("Project {id}"),
        stage: Stage::InReview,
        lead: Cid(109),
        paused: false,
        budget: Some(12.5),
        limits: Limits {
            turns: 40,
            seconds: None,
        },
        auth: Auth::Key {
            env: "KEY".into(),
            header: Some("x-api-key".into()),
        },
        extra: Some(Limits {
            turns: 3,
            seconds: Some(9),
        }),
        docs: vec![
            Doc {
                id: "d1".into(),
                title: "Spec".into(),
                notes: vec![note(1, "first", &["a", "b"]), note(2, "second", &[])],
            },
            Doc {
                id: "d2".into(),
                title: "Plan".into(),
                notes: vec![],
            },
        ],
        cover: Some(note(5, "cover", &["c"])),
        tasks: vec!["t1".into(), "t2".into()],
        members: [300, 301].into_iter().collect(),
        by_name: [("x".to_string(), note(7, "seven", &["s"]))]
            .into_iter()
            .collect(),
        labels: [("p".to_string(), 1), ("q".to_string(), -2)]
            .into_iter()
            .collect(),
        prompt: "{\"as\": \"written\"}".into(),
        cache: vec![1, 2, 3],
    }
}

fn migrated(def: &apz_store::TableDef) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    def.validate().unwrap();
    conn.execute_batch(&def.ddl()).unwrap();
    conn
}

#[test]
fn a_struct_round_trips_through_its_tables() {
    let def = Project::root();
    let conn = migrated(&def);
    let items = vec![project("pr2"), project("pr10")];
    let rows = RowSet::list(&def, &items).unwrap();
    table::replace(&conn, &def, &rows).unwrap();
    let back: Vec<Project> = load_list(&conn, &def).unwrap();
    let mut want = items.clone();
    for p in &mut want {
        p.cache = Vec::new();
    }
    assert_eq!(
        back, want,
        "the list's order and every field survive; skipped fields come back empty"
    );
}

#[test]
fn the_schema_is_strict_typed_and_passes_the_lint() {
    let def = Project::root();
    let conn = migrated(&def);
    assert_eq!(lint::schema(&conn).unwrap(), Vec::<String>::new());
    let ddl = def.ddl();
    assert!(
        ddl.contains("\"stage\" TEXT NOT NULL CHECK (\"stage\" IN ('draft', 'in_review', 'done'))"),
        "{ddl}"
    );
    assert!(
        ddl.contains("\"paused\" INTEGER NOT NULL CHECK (\"paused\" IN (0, 1))"),
        "{ddl}"
    );
    assert!(ddl.contains("\"auth_kind\" TEXT NOT NULL"), "{ddl}");
    assert!(ddl.contains("\"prompt_content\" TEXT"), "{ddl}");
    assert!(
        ddl.contains("CREATE TABLE \"project_docs_notes_tags\""),
        "{ddl}"
    );
    assert!(
        ddl.contains("PRIMARY KEY (\"project_id\", \"docs_pos\", \"notes_pos\", \"tags_pos\")"),
        "{ddl}"
    );
    for stmt in def.statements() {
        conn.prepare(&stmt)
            .unwrap_or_else(|e| panic!("{stmt}: {e}"));
        assert!(lint::sql("generated", &stmt).is_empty(), "{stmt}");
    }
}

#[test]
fn a_check_refuses_a_value_outside_the_enum() {
    let def = Project::root();
    let conn = migrated(&def);
    let rows = RowSet::list(&def, &[project("pr1")]).unwrap();
    table::replace(&conn, &def, &rows).unwrap();
    let err = conn
        .execute("UPDATE project SET stage = 'nope'", [])
        .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");
}

#[test]
fn saves_write_only_what_changed_and_deletes_cascade() {
    let def = Project::root();
    let conn = migrated(&def);
    let mut items = vec![project("pr1"), project("pr2")];
    let rows = RowSet::list(&def, &items).unwrap();
    let mut saved = Saved::default();
    let first = saved.diff(&def, &rows);
    table::apply(&conn, &def, &first).unwrap();
    let total = first.upserts.len();
    saved.commit(first);

    // One field of one project changes: one upsert.
    items[1].title = "Renamed".into();
    let rows = RowSet::list(&def, &items).unwrap();
    let change = saved.diff(&def, &rows);
    assert_eq!(
        (change.upserts.len(), change.deletes.len()),
        (1, 0),
        "{change:?}"
    );
    table::apply(&conn, &def, &change).unwrap();
    saved.commit(change);

    // A note removed from a doc: its row and its tags' rows are deleted.
    items[0].docs[0].notes.truncate(1);
    let rows = RowSet::list(&def, &items).unwrap();
    let change = saved.diff(&def, &rows);
    assert!(change.upserts.is_empty(), "{change:?}");
    assert_eq!(change.deletes.len(), 1, "the note's row; it had no tags");
    table::apply(&conn, &def, &change).unwrap();
    saved.commit(change);

    // A project removed: its row goes and its children cascade.
    items.remove(0);
    let rows = RowSet::list(&def, &items).unwrap();
    let change = saved.diff(&def, &rows);
    table::apply(&conn, &def, &change).unwrap();
    saved.commit(change);
    let back: Vec<Project> = load_list(&conn, &def).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].title, "Renamed");
    let left: i64 = conn
        .query_row(
            "SELECT count(*) FROM project_docs WHERE project_id = 'pr1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "the removed project's docs went with it");
    assert!(total > 10, "the first save wrote every row ({total})");
}

#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "settings")]
struct Settings {
    theme: String,
    #[table(map_value)]
    flags: BTreeMap<String, bool>,
}

#[test]
fn a_single_value_round_trips() {
    let def = Settings::root();
    let conn = migrated(&def);
    let value = Settings {
        theme: "loom".into(),
        flags: [("a".to_string(), true)].into_iter().collect(),
    };
    table::replace(&conn, &def, &RowSet::one(&def, &value).unwrap()).unwrap();
    assert_eq!(load_one::<Settings>(&conn, &def).unwrap(), Some(value));
}

#[derive(Debug, Clone, PartialEq, Table)]
enum Event {
    Started {
        #[table(flatten)]
        limits: Limits,
        #[table(content)]
        prompt: String,
    },
    Ended {
        ok: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "log")]
struct Log {
    at_ms: u64,
    #[table(flatten)]
    event: Event,
}

#[test]
fn an_enum_variant_may_flatten_a_struct() {
    let def = Log::root();
    let conn = migrated(&def);
    let rows = vec![
        Log {
            at_ms: 1,
            event: Event::Started {
                limits: Limits {
                    turns: 3,
                    seconds: Some(4),
                },
                prompt: "go".into(),
            },
        },
        Log {
            at_ms: 2,
            event: Event::Ended { ok: true },
        },
    ];
    table::replace(&conn, &def, &RowSet::list(&def, &rows).unwrap()).unwrap();
    assert_eq!(load_list::<Log>(&conn, &def).unwrap(), rows);
    assert!(
        def.ddl()
            .contains("\"event_started_limits_turns\" INTEGER CHECK"),
        "{}",
        def.ddl()
    );
}

#[test]
fn a_u64_past_i64_is_refused_not_wrapped() {
    assert!(u64::MAX.to_value().is_err());
    assert_eq!(
        Column::to_value(&7u64).unwrap(),
        apz_store::table::Value::Integer(7)
    );
}
