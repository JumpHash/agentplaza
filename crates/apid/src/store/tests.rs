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
