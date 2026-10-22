//! The schema lint (W69): no JSON in the store.
//!
//! [`schema`] fails a database with a table that is not `STRICT`, a column without a concrete type (`ANY`), or a
//! column whose name says it holds JSON. [`sql`] fails a query that calls a JSON function or uses the JSON operators.
//! Content (a message's text, a tool's input as the model wrote it) is stored in columns named `…_content` and is
//! never read back as state; the lint allows those.

use rusqlite::Connection;

use crate::StoreError;

/// The column types a `STRICT` table may declare that the lint accepts (`ANY` is refused: every column has a type).
const TYPES: &[&str] = &["INTEGER", "INT", "REAL", "TEXT", "BLOB"];

/// Every problem the lint finds in the database open as `conn`, as one line each; empty when it passes.
pub fn schema(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut problems = Vec::new();
    let mut tables = conn.prepare(
        "SELECT name, type, strict FROM pragma_table_list WHERE schema = 'main' ORDER BY name",
    )?;
    let rows = tables.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (name, kind, strict) = row?;
        // SQLite's own tables, FTS5's virtual tables and their shadow tables are SQLite's, not ours.
        if name.starts_with("sqlite_") || kind != "table" {
            continue;
        }
        if strict != 1 {
            problems.push(format!("table {name} is not STRICT"));
        }
        let mut cols = conn.prepare("SELECT name, type FROM pragma_table_xinfo(?1)")?;
        let cols = cols.query_map([&name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for col in cols {
            let (col, ty) = col?;
            let ty = ty.to_ascii_uppercase();
            if !TYPES.contains(&ty.as_str()) {
                problems.push(format!(
                    "column {name}.{col} has type {ty:?}, not one of {TYPES:?}"
                ));
            }
            let lower = col.to_ascii_lowercase();
            if lower.contains("json") {
                problems.push(format!(
                    "column {name}.{col} is named as JSON: store typed columns instead"
                ));
            }
        }
    }
    Ok(problems)
}

/// Every problem the lint finds in one query's text (`name` is how it is reported).
pub fn sql(name: &str, text: &str) -> Vec<String> {
    let code = strip_comments_and_strings(text).to_ascii_lowercase();
    let mut problems = Vec::new();
    if code.contains("json") {
        problems.push(format!(
            "query {name} uses JSON (a json function or type): read typed columns instead"
        ));
    }
    if code.contains("->") {
        problems.push(format!("query {name} uses the JSON operator ->"));
    }
    problems
}

/// The query's code with `--` and `/* */` comments and quoted strings blanked, so a word inside them is not code.
fn strip_comments_and_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                out.push(' ');
            }
            '\'' => {
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                }
                out.push_str("''");
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strict_typed_schema_passes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE a (id INTEGER PRIMARY KEY, name TEXT NOT NULL, body_content TEXT) STRICT;
             CREATE VIRTUAL TABLE a_search USING fts5(name);",
        )
        .unwrap();
        assert_eq!(schema(&conn).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn loose_tables_any_columns_and_json_names_fail() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE loose (id INTEGER PRIMARY KEY, data TEXT);
             CREATE TABLE anything (id INTEGER PRIMARY KEY, v ANY) STRICT;
             CREATE TABLE named (id INTEGER PRIMARY KEY, meta_json TEXT) STRICT;",
        )
        .unwrap();
        let problems = schema(&conn).unwrap();
        assert!(
            problems.iter().any(|p| p.contains("loose is not STRICT")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("anything.v")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("named.meta_json")),
            "{problems:?}"
        );
    }

    #[test]
    fn json_in_queries_fails_but_not_in_comments_or_strings() {
        assert!(
            sql(
                "a",
                "SELECT name FROM a -- json is not used here\nWHERE x = 'json'"
            )
            .is_empty()
        );
        assert_eq!(sql("b", "SELECT json_extract(data, '$.x') FROM a").len(), 1);
        assert_eq!(sql("c", "SELECT data ->> 'x' FROM a").len(), 1);
        assert!(sql("d", "/* json */ SELECT 1").is_empty());
    }
}
