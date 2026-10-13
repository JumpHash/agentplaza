//! Every SQL statement apid runs. Each lives in its own file under `crates/apid/sql/queries/`
//! and is embedded at compile time; the file's first comment says what it does and its `$n`
//! parameters. Adding a query: write `sql/queries/<area>_<verb>.sql`, then one `pub const` here
//! (docs/dev/sql.md). The test below fails when a file and its const disagree.

// The state snapshots (`plaza_state`).
pub const STATE_GET: &str = include_str!("../../sql/queries/state_get.sql");
pub const STATE_UPSERT: &str = include_str!("../../sql/queries/state_upsert.sql");
#[cfg(test)]
pub const STATE_DELETE: &str = include_str!("../../sql/queries/state_delete.sql");

// The transcripts (`transcript`).
pub const TRANSCRIPT_APPEND: &str = include_str!("../../sql/queries/transcript_append.sql");
pub const TRANSCRIPT_PAGE: &str = include_str!("../../sql/queries/transcript_page.sql");
pub const TRANSCRIPT_CIDS: &str = include_str!("../../sql/queries/transcript_cids.sql");
pub const TRANSCRIPT_DELETE: &str = include_str!("../../sql/queries/transcript_delete.sql");

// The infractions (`infraction`).
pub const INFRACTION_APPEND: &str = include_str!("../../sql/queries/infraction_append.sql");
pub const INFRACTION_PAGE_BY_CID: &str = include_str!("../../sql/queries/infraction_page_by_cid.sql");
pub const INFRACTION_PAGE: &str = include_str!("../../sql/queries/infraction_page.sql");

// The migration runner's bookkeeping (`schema_migrations`, the advisory lock).
pub const MIGRATION_TABLE_ENSURE: &str = include_str!("../../sql/queries/migration_table_ensure.sql");
pub const MIGRATION_APPLIED: &str = include_str!("../../sql/queries/migration_applied.sql");
pub const MIGRATION_RECORD: &str = include_str!("../../sql/queries/migration_record.sql");
pub const MIGRATION_LOCK: &str = include_str!("../../sql/queries/migration_lock.sql");
pub const MIGRATION_UNLOCK: &str = include_str!("../../sql/queries/migration_unlock.sql");

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    /// This file's own source: every `pub const` line must include a file of the queries folder.
    const SOURCE: &str = include_str!("sql.rs");

    /// Every file in `sql/queries` has exactly one const, every const includes a file of that
    /// folder, and every file is a non-empty statement that opens with a `--` comment.
    #[test]
    fn every_query_file_has_a_const_and_every_const_has_a_file() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("sql").join("queries");
        let files: BTreeSet<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|entry| entry.expect("directory entry").file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".sql"))
            .collect();

        let mut consts = 0;
        let mut included = BTreeSet::new();
        for line in SOURCE.lines().filter(|line| line.starts_with("pub const ")) {
            consts += 1;
            let file = line
                .split("sql/queries/")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("const is not an include of a queries file: {line}"));
            assert!(included.insert(file.to_string()), "two consts include {file}");
        }

        let orphans: Vec<&String> = files.difference(&included).collect();
        let missing: Vec<&String> = included.difference(&files).collect();
        assert!(orphans.is_empty(), "query files with no const in db/sql.rs: {orphans:?}");
        assert!(missing.is_empty(), "consts with no query file: {missing:?}");
        assert_eq!(consts, files.len());

        for file in &files {
            let text = std::fs::read_to_string(dir.join(file)).expect("read query file");
            assert!(!text.trim().is_empty(), "{file} is empty");
            assert!(text.starts_with("--"), "{file} must open with a -- comment naming its parameters");
        }
    }
}
