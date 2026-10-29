//! The schema's checks: every component's tables are in `0001_init.sql` exactly as their definitions say, every file
//! passes the lint (STRICT, typed, no JSON), and every query prepares against the file it runs on.

use apz_store::rusqlite::Connection;
use apz_store::{FileSpec, lint};

use super::{FILES, components, sql};

/// A table set's name in the messages.
struct Named<'a> {
    name: &'a str,
}

fn migrated(spec: &FileSpec) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    for m in spec.migrations {
        conn.execute_batch(m.sql).unwrap_or_else(|e| {
            panic!(
                "{} migration {} ({}) fails: {e}",
                spec.file, m.version, m.name
            )
        });
    }
    conn
}

/// The columns of `table` in a migrated file: (name, type, not null, primary key position).
fn columns(conn: &Connection, table: &str) -> Vec<(String, String, bool, i64)> {
    let mut stmt = conn
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)")
        .unwrap();
    stmt.query_map([table], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1, r.get(3)?))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

#[test]
fn every_file_migrates_and_passes_the_lint() {
    for spec in FILES {
        let conn = migrated(spec);
        let problems = lint::schema(&conn).unwrap();
        assert!(problems.is_empty(), "{}: {problems:#?}", spec.file);
    }
}

#[test]
fn local_provider_upgrade_preserves_existing_rows_and_foreign_keys() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute_batch(super::PLAZA_MIGRATIONS[0].sql).unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO providers VALUES (0);
        INSERT INTO provider (providers_pos, providers_pos_2, id, kind, enabled, auth_kind, auth_secret, label, base_url, extra_body_content)
            VALUES (0, 0, 'existing', 'openai-compatible', 1, 'secret', 'stored-key-name', 'Existing provider', 'https://example.invalid/v1', '{"safe":true}');
        INSERT INTO provider_extra_headers VALUES (0, 0, 'X-Client', 'test');
        INSERT INTO provider_models VALUES (0, 0, 0, 'model-a'), (0, 0, 1, 'model-b');
        INSERT INTO provider_options (providers_pos, providers_pos_2, region, project, websocket, models_cache_secs, reasoning_effort)
            VALUES (0, 0, 'eu-west-1', 'project-a', 0, 42, 1);
        INSERT INTO provider_options_trains_on_data VALUES (0, 0, 0, 'model-b');
        INSERT INTO provider_offered VALUES (0, 0, 'existing/model-a');
        "#,
    ).unwrap();
    let tables = [
        "providers",
        "provider",
        "provider_extra_headers",
        "provider_models",
        "provider_options",
        "provider_options_trains_on_data",
        "provider_offered",
    ];
    let snapshots: Vec<_> = tables
        .iter()
        .map(|table| {
            let names: Vec<_> = columns(&conn, table)
                .into_iter()
                .map(|column| format!("\"{}\"", column.0))
                .collect();
            let sql = format!("SELECT {} FROM \"{table}\" ORDER BY 1, 2", names.join(", "));
            // The root table has only one column.
            let sql = if names.len() == 1 {
                sql.replace("ORDER BY 1, 2", "ORDER BY 1")
            } else {
                sql
            };
            let read = |conn: &Connection| {
                let mut stmt = conn.prepare(&sql).unwrap();
                stmt.query_map([], |row| {
                    (0..names.len())
                        .map(|index| row.get::<_, apz_store::rusqlite::types::Value>(index))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
            };
            let expected = read(&conn);
            (sql, expected)
        })
        .collect();
    let tx = conn.transaction().unwrap();
    tx.execute_batch(super::PLAZA_MIGRATIONS[1].sql).unwrap();
    tx.commit().unwrap();
    for (sql, expected) in snapshots {
        let mut stmt = conn.prepare(&sql).unwrap();
        let count = stmt.column_count();
        let actual = stmt
            .query_map([], |row| {
                (0..count)
                    .map(|index| row.get::<_, apz_store::rusqlite::types::Value>(index))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(actual, expected, "{sql}");
    }
    assert_eq!(
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, u64>(
            0
        ))
        .unwrap(),
        0
    );
    assert_eq!(conn.query_row("SELECT count(*) FROM provider_options WHERE executable IS NULL AND working_directory IS NULL", [], |r| r.get::<_, u64>(0)).unwrap(), 1);
    for kind in ["codex-app-server", "opencode-local"] {
        assert_eq!(
            conn.execute("UPDATE provider SET kind = ?1", [kind])
                .unwrap(),
            1
        );
    }
    assert!(
        conn.execute("UPDATE provider SET kind = 'unknown'", [])
            .is_err()
    );
    assert!(lint::schema(&conn).unwrap().is_empty());
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_temp_master WHERE name LIKE '_local_upgrade_%'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn every_component_table_is_in_the_migration_as_defined() {
    let plaza = FILES.iter().find(|f| f.name == super::PLAZA).unwrap();
    let conn = migrated(plaza);
    let mut missing = Vec::new();
    let tables: Vec<(&str, &apz_store::TableDef)> = components::all()
        .into_iter()
        .map(|c| (c.name, (c.def)()))
        .chain(
            components::written_through()
                .into_iter()
                .map(|(n, d)| (n, d())),
        )
        .collect();
    for (name, def) in tables {
        let c = Named { name };
        def.validate()
            .unwrap_or_else(|e| panic!("component {}: {e}", c.name));
        // The definition's own DDL into a scratch database: what the table must look like.
        let want = Connection::open_in_memory().unwrap();
        want.execute_batch(&def.ddl()).unwrap();
        for table in def.walk() {
            let have = columns(&conn, &table.name);
            if have.is_empty() {
                missing.push(format!(
                    "component {}: table {} is not in 0001_init.sql",
                    c.name, table.name
                ));
                continue;
            }
            let expected = columns(&want, &table.name);
            if have != expected {
                missing.push(format!(
                    "component {}: table {} differs from its definition:\n  migration {have:?}\n  definition {expected:?}",
                    c.name, table.name
                ));
            }
        }
        for stmt in def.statements() {
            conn.prepare(&stmt)
                .unwrap_or_else(|e| panic!("component {}: {stmt}: {e}", c.name));
        }
    }
    assert!(
        missing.is_empty(),
        "{}\n\nRegenerate the components' part of sql/plaza/0001_init.sql with `cargo run -p apid -- schema`.",
        missing.join("\n")
    );
}

#[test]
fn every_query_prepares_against_its_file_and_passes_the_lint() {
    let files: Vec<(&str, Connection)> = FILES.iter().map(|f| (f.name, migrated(f))).collect();
    for (file, name, text) in sql::QUERIES {
        let conn = &files
            .iter()
            .find(|(n, _)| n == file)
            .unwrap_or_else(|| panic!("query {name}: no file {file}"))
            .1;
        conn.prepare(text)
            .unwrap_or_else(|e| panic!("query {name} ({file}): {e}"));
        let problems = lint::sql(name, text);
        assert!(problems.is_empty(), "{problems:?}");
    }
}

/// Every file of `sql/queries` is included by exactly one const of `store/sql.rs`, every include is a file of the folder,
/// and every file is a statement that opens with a `--` comment naming its parameters (docs/dev/sql.md).
#[test]
fn every_query_file_has_one_const_and_every_const_has_a_file() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("sql")
        .join("queries");
    let files: std::collections::BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".sql"))
        .collect();
    let included: Vec<String> = include_str!("sql.rs")
        .split("include_str!(\"../../sql/queries/")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap_or_default().to_string())
        .collect();
    let distinct: std::collections::BTreeSet<&String> = included.iter().collect();
    assert_eq!(
        distinct.len(),
        included.len(),
        "a query file is included twice"
    );
    let included: std::collections::BTreeSet<String> = included.into_iter().collect();
    let orphans: Vec<&String> = files.difference(&included).collect();
    let missing: Vec<&String> = included.difference(&files).collect();
    assert!(
        orphans.is_empty(),
        "query files with no const in store/sql.rs: {orphans:?}"
    );
    assert!(missing.is_empty(), "consts with no query file: {missing:?}");
    for file in &files {
        let text = std::fs::read_to_string(dir.join(file)).expect("read query file");
        assert!(!text.trim().is_empty(), "{file} is empty");
        assert!(
            text.starts_with("--"),
            "{file} must open with a -- comment naming its parameters"
        );
    }
}
