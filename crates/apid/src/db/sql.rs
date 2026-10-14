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
pub const INFRACTION_PAGE_BY_CID: &str =
    include_str!("../../sql/queries/infraction_page_by_cid.sql");
pub const INFRACTION_PAGE: &str = include_str!("../../sql/queries/infraction_page.sql");

// The IDE audit (`ide_audit`, migration 0008; ide.rs owner actions).
pub const IDE_AUDIT_APPEND: &str = include_str!("../../sql/queries/ide_audit_append.sql");
pub const IDE_AUDIT_PAGE: &str = include_str!("../../sql/queries/ide_audit_page.sql");
pub const IDE_AUDIT_PAGE_BY_WORKSPACE: &str =
    include_str!("../../sql/queries/ide_audit_page_by_workspace.sql");

// The station audit (`station_audit`, migration 0010; stations/ owner actions and lease changes).
pub const STATION_AUDIT_APPEND: &str = include_str!("../../sql/queries/station_audit_append.sql");
pub const STATION_AUDIT_PAGE: &str = include_str!("../../sql/queries/station_audit_page.sql");
pub const STATION_AUDIT_PAGE_BY_STATION: &str =
    include_str!("../../sql/queries/station_audit_page_by_station.sql");

// The workstation recordings (`station_recordings` and `station_recording_events`, migration 0011; stations/recordings.rs).
pub const STATION_RECORDING_PUT: &str = include_str!("../../sql/queries/station_recording_put.sql");
pub const STATION_RECORDING_GET: &str = include_str!("../../sql/queries/station_recording_get.sql");
pub const STATION_RECORDING_PAGE: &str =
    include_str!("../../sql/queries/station_recording_page.sql");
pub const STATION_RECORDING_EXPIRED: &str =
    include_str!("../../sql/queries/station_recording_expired.sql");
pub const STATION_RECORDING_DELETE: &str =
    include_str!("../../sql/queries/station_recording_delete.sql");
pub const STATION_RECORDING_EVENT_APPEND: &str =
    include_str!("../../sql/queries/station_recording_event_append.sql");
pub const STATION_RECORDING_EVENTS_PAGE: &str =
    include_str!("../../sql/queries/station_recording_events_page.sql");
// The frames' capture times (`station_recording_frames`, migration 0013; stations/recordings.rs).
pub const STATION_RECORDING_FRAME_PUT: &str =
    include_str!("../../sql/queries/station_recording_frame_put.sql");
pub const STATION_RECORDING_FRAMES_PAGE: &str =
    include_str!("../../sql/queries/station_recording_frames_page.sql");

// The work-folder sync's mirror (`station_sync_files`, migration 0012; stations/sync.rs).
pub const STATION_SYNC_ROWS: &str = include_str!("../../sql/queries/station_sync_rows.sql");
pub const STATION_SYNC_SAVE: &str = include_str!("../../sql/queries/station_sync_save.sql");
// The unfurl cache (`unfurl_cache`, migration 0009; unfurl/mod.rs).
pub const UNFURL_GET: &str = include_str!("../../sql/queries/unfurl_get.sql");
pub const UNFURL_PUT: &str = include_str!("../../sql/queries/unfurl_put.sql");

// The migration runner's bookkeeping (`schema_migrations`, the advisory lock).
pub const MIGRATION_TABLE_ENSURE: &str =
    include_str!("../../sql/queries/migration_table_ensure.sql");
pub const MIGRATION_APPLIED: &str = include_str!("../../sql/queries/migration_applied.sql");
pub const MIGRATION_RECORD: &str = include_str!("../../sql/queries/migration_record.sql");
pub const MIGRATION_LOCK: &str = include_str!("../../sql/queries/migration_lock.sql");
pub const MIGRATION_UNLOCK: &str = include_str!("../../sql/queries/migration_unlock.sql");

// The data migration (`data_migrations`, docs/design/database.md section 4).
pub const AGENTS_SOURCE: &str = include_str!("../../sql/queries/agents_source.sql");
pub const AGENTS_DELETE: &str = include_str!("../../sql/queries/agents_delete.sql");
pub const AGENTS_INSERT: &str = include_str!("../../sql/queries/agents_insert.sql");
pub const AGENTS_COUNT: &str = include_str!("../../sql/queries/agents_count.sql");

// The roster's write-through to the agents table (db/roster.rs).
pub const AGENTS_UPSERT: &str = include_str!("../../sql/queries/agents_upsert.sql");
pub const AGENTS_RETIRE: &str = include_str!("../../sql/queries/agents_retire.sql");
pub const AGENTS_ACTIVE: &str = include_str!("../../sql/queries/agents_active.sql");
pub const DATA_MIGRATION_STATUS: &str = include_str!("../../sql/queries/data_migration_status.sql");
pub const DATA_MIGRATION_RECORD: &str = include_str!("../../sql/queries/data_migration_record.sql");
pub const DATA_MIGRATION_FAIL: &str = include_str!("../../sql/queries/data_migration_fail.sql");

// The hub's MCP server (hub_mcp_*: the tokens, the audit log and the settings row; docs/design/hub-mcp.md).
pub const HUB_MCP_SETTINGS_GET: &str = include_str!("../../sql/queries/hub_mcp_settings_get.sql");
pub const HUB_MCP_SETTINGS_PUT: &str = include_str!("../../sql/queries/hub_mcp_settings_put.sql");
pub const HUB_MCP_TOKEN_INSERT: &str = include_str!("../../sql/queries/hub_mcp_token_insert.sql");
pub const HUB_MCP_TOKEN_BY_DIGEST: &str =
    include_str!("../../sql/queries/hub_mcp_token_by_digest.sql");
pub const HUB_MCP_TOKEN_LIST: &str = include_str!("../../sql/queries/hub_mcp_token_list.sql");
pub const HUB_MCP_TOKEN_USE: &str = include_str!("../../sql/queries/hub_mcp_token_use.sql");
pub const HUB_MCP_TOKEN_REVOKE: &str = include_str!("../../sql/queries/hub_mcp_token_revoke.sql");
pub const HUB_MCP_CALL_INSERT: &str = include_str!("../../sql/queries/hub_mcp_call_insert.sql");
pub const HUB_MCP_CALL_PAGE: &str = include_str!("../../sql/queries/hub_mcp_call_page.sql");

// The prompt library (prompt_*: migration 0006, docs/design/prompt-library.md; prompts/store_db.rs).
pub const PROMPT_GET: &str = include_str!("../../sql/queries/prompt_get.sql");
pub const PROMPT_LOCK: &str = include_str!("../../sql/queries/prompt_lock.sql");
pub const PROMPT_LIST: &str = include_str!("../../sql/queries/prompt_list.sql");
pub const PROMPT_VERSIONS: &str = include_str!("../../sql/queries/prompt_versions.sql");
pub const PROMPT_VERSION_GET: &str = include_str!("../../sql/queries/prompt_version_get.sql");
pub const PROMPT_FINDINGS: &str = include_str!("../../sql/queries/prompt_findings.sql");
pub const PROMPT_EVENTS: &str = include_str!("../../sql/queries/prompt_events.sql");
pub const PROMPT_BOUND: &str = include_str!("../../sql/queries/prompt_bound.sql");
pub const PROMPT_BINDING_HOLDER: &str = include_str!("../../sql/queries/prompt_binding_holder.sql");
pub const PROMPT_INSERT: &str = include_str!("../../sql/queries/prompt_insert.sql");
pub const PROMPT_VERSION_INSERT: &str = include_str!("../../sql/queries/prompt_version_insert.sql");
pub const PROMPT_FINDING_INSERT: &str = include_str!("../../sql/queries/prompt_finding_insert.sql");
pub const PROMPT_EVENT_INSERT: &str = include_str!("../../sql/queries/prompt_event_insert.sql");
pub const PROMPT_SET_LATEST: &str = include_str!("../../sql/queries/prompt_set_latest.sql");
pub const PROMPT_SET_META: &str = include_str!("../../sql/queries/prompt_set_meta.sql");
pub const PROMPT_SET_ACTIVE: &str = include_str!("../../sql/queries/prompt_set_active.sql");
pub const PROMPT_SET_ARCHIVED: &str = include_str!("../../sql/queries/prompt_set_archived.sql");

// The skill library (`skills`, migration 0007, docs/design/skill-library.md).
pub const SKILL_LIST: &str = include_str!("../../sql/queries/skill_list.sql");
pub const SKILL_GET: &str = include_str!("../../sql/queries/skill_get.sql");
pub const SKILL_LOCK: &str = include_str!("../../sql/queries/skill_lock.sql");
pub const SKILL_NAME_HOLDER: &str = include_str!("../../sql/queries/skill_name_holder.sql");
pub const SKILL_VERSIONS: &str = include_str!("../../sql/queries/skill_versions.sql");
pub const SKILL_VERSION_GET: &str = include_str!("../../sql/queries/skill_version_get.sql");
pub const SKILL_FILES: &str = include_str!("../../sql/queries/skill_files.sql");
pub const SKILL_FINDINGS: &str = include_str!("../../sql/queries/skill_findings.sql");
pub const SKILL_EVENTS: &str = include_str!("../../sql/queries/skill_events.sql");
pub const SKILL_BOUND: &str = include_str!("../../sql/queries/skill_bound.sql");
pub const SKILL_INSERT: &str = include_str!("../../sql/queries/skill_insert.sql");
pub const SKILL_VERSION_INSERT: &str = include_str!("../../sql/queries/skill_version_insert.sql");
pub const SKILL_FILE_INSERT: &str = include_str!("../../sql/queries/skill_file_insert.sql");
pub const SKILL_FINDING_INSERT: &str = include_str!("../../sql/queries/skill_finding_insert.sql");
pub const SKILL_EVENT_INSERT: &str = include_str!("../../sql/queries/skill_event_insert.sql");
pub const SKILL_SET_LATEST: &str = include_str!("../../sql/queries/skill_set_latest.sql");
pub const SKILL_SET_META: &str = include_str!("../../sql/queries/skill_set_meta.sql");
pub const SKILL_TOUCH: &str = include_str!("../../sql/queries/skill_touch.sql");
pub const SKILL_SET_ACTIVE: &str = include_str!("../../sql/queries/skill_set_active.sql");
pub const SKILL_SET_ARCHIVED: &str = include_str!("../../sql/queries/skill_set_archived.sql");
pub const SKILL_BINDINGS_DELETE: &str = include_str!("../../sql/queries/skill_bindings_delete.sql");
pub const SKILL_BINDINGS_INSERT: &str = include_str!("../../sql/queries/skill_bindings_insert.sql");

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
