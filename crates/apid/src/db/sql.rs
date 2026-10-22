//! Every SQL statement apid runs. Each lives in its own file under `crates/apid/sql/queries/`
//! and is embedded at compile time; the file's first comment says what it does and its `$n`
//! parameters. Adding a query: write `sql/queries/<area>_<verb>.sql`, then one `pub const` here
//! (docs/dev/sql.md). The test below fails when a file and its const disagree.

// The state snapshots (`plaza_state`).
pub const STATE_GET: &str = include_str!("../../sql/queries/state_get.sql");
pub const STATE_UPSERT: &str = include_str!("../../sql/queries/state_upsert.sql");
#[cfg(test)]
pub const STATE_DELETE: &str = include_str!("../../sql/queries/state_delete.sql");

// The migration runner's bookkeeping (`schema_migrations`, the advisory lock).
pub const MIGRATION_TABLE_ENSURE: &str =
    include_str!("../../sql/queries/migration_table_ensure.sql");
pub const MIGRATION_APPLIED: &str = include_str!("../../sql/queries/migration_applied.sql");
pub const MIGRATION_RECORD: &str = include_str!("../../sql/queries/migration_record.sql");
pub const MIGRATION_LOCK: &str = include_str!("../../sql/queries/migration_lock.sql");
pub const MIGRATION_UNLOCK: &str = include_str!("../../sql/queries/migration_unlock.sql");

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    /// This file's own source: every `pub const` declaration must include a file of the queries folder.
    const SOURCE: &str = include_str!("sql.rs");

    /// The declarations of `SOURCE`, each joined into one line. rustfmt may break a long
    /// `include_str!` declaration after its `=`, so a declaration runs from its `pub const` line to
    /// the line that ends with `;`.
    fn declarations() -> Vec<String> {
        let mut found = Vec::new();
        let mut open = String::new();
        for line in SOURCE.lines() {
            if open.is_empty() && !line.starts_with("pub const ") {
                continue;
            }
            open.push_str(line.trim());
            open.push(' ');
            if line.trim_end().ends_with(';') {
                found.push(std::mem::take(&mut open).trim_end().to_string());
            }
        }
        assert!(open.is_empty(), "a const declaration never ends: {open}");
        found
    }

    /// Every file in `sql/queries` has exactly one const, every const includes a file of that
    /// folder, and every file is a non-empty statement that opens with a `--` comment.
    #[test]
    fn every_query_file_has_a_const_and_every_const_has_a_file() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("sql")
            .join("queries");
        let files: BTreeSet<String> = std::fs::read_dir(&dir)
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

        let declarations = declarations();
        let consts = declarations.len();
        let mut included = BTreeSet::new();
        for declaration in &declarations {
            let file = declaration
                .split("sql/queries/")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| {
                    panic!("const is not an include of a queries file: {declaration}")
                });
            assert!(
                included.insert(file.to_string()),
                "two consts include {file}"
            );
        }

        // The SQLite queries (store/sql.rs) share the folder while Postgres is replaced: they are not orphans here.
        let sqlite = include_str!("../store/sql.rs");
        let files: BTreeSet<String> = files
            .into_iter()
            .filter(|f| !sqlite.contains(&format!("sql/queries/{f}")))
            .collect();
        let orphans: Vec<&String> = files.difference(&included).collect();
        let missing: Vec<&String> = included.difference(&files).collect();
        assert!(
            orphans.is_empty(),
            "query files with no const in db/sql.rs: {orphans:?}"
        );
        assert!(missing.is_empty(), "consts with no query file: {missing:?}");
        assert_eq!(consts, files.len());

        for file in &files {
            let text = std::fs::read_to_string(dir.join(file)).expect("read query file");
            assert!(!text.trim().is_empty(), "{file} is empty");
            assert!(
                text.starts_with("--"),
                "{file} must open with a -- comment naming its parameters"
            );
        }
    }
}
