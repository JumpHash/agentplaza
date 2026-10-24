//! Every SQL statement the hub runs by hand (the components' statements come from their table definitions). Each
//! lives in its own file under `crates/apid/sql/queries/`, embedded at compile time, behind one `pub const` here and a
//! line in [`QUERIES`] naming the file it runs against; the test prepares every one against its migrated file and
//! runs the lint on it (docs/dev/sql.md).

// The budgets' ledger (budgets.rs).
pub const BUDGET_LEDGER: &str = include_str!("../../sql/queries/budget_ledger.sql");

// The model-usage ledger (usage.rs): written through as calls are metered, read and pruned by the retention.
pub const USAGE_CALLS_CLEAR: &str = include_str!("../../sql/queries/usage_calls_clear.sql");
pub const USAGE_CALLS_COUNT: &str = include_str!("../../sql/queries/usage_calls_count.sql");
pub const USAGE_CALLS_PAGE: &str = include_str!("../../sql/queries/usage_calls_page.sql");
pub const USAGE_CALLS_PRUNE: &str = include_str!("../../sql/queries/usage_calls_prune.sql");
pub const USAGE_ESCALATED: &str = include_str!("../../sql/queries/usage_escalated.sql");
pub const USAGE_HOUR_ADD: &str = include_str!("../../sql/queries/usage_hour_add.sql");
pub const USAGE_HOURS_PRUNE: &str = include_str!("../../sql/queries/usage_hours_prune.sql");
pub const USAGE_NEWEST_ID: &str = include_str!("../../sql/queries/usage_newest_id.sql");
pub const USAGE_SERIES: &str = include_str!("../../sql/queries/usage_series.sql");
pub const USAGE_SUMMARY: &str = include_str!("../../sql/queries/usage_summary.sql");
pub const USAGE_WORK_ADD: &str = include_str!("../../sql/queries/usage_work_add.sql");
pub const USAGE_WORK_TOTALS: &str = include_str!("../../sql/queries/usage_work_totals.sql");
pub const USAGE_WORKS_PRUNE: &str = include_str!("../../sql/queries/usage_works_prune.sql");

// The eval records (evals.rs): a page of runs, the scores, and the landed flag of a task's runs.
pub const EVAL_RUNS_PAGE: &str = include_str!("../../sql/queries/eval_runs_page.sql");
pub const EVAL_SCORES: &str = include_str!("../../sql/queries/eval_scores.sql");
pub const EVAL_RUNS_LANDED: &str = include_str!("../../sql/queries/eval_runs_landed.sql");
pub const EVAL_RUNS_ALL: &str = include_str!("../../sql/queries/eval_runs_all.sql");
pub const EVAL_RUNS_REVERTED: &str = include_str!("../../sql/queries/eval_runs_reverted.sql");
pub const EVAL_RUNS_INCIDENT: &str = include_str!("../../sql/queries/eval_runs_incident.sql");
pub const EVAL_RUNS_QUALIFIED: &str = include_str!("../../sql/queries/eval_runs_qualified.sql");
pub const EVAL_RUNS_LATER_FINDING: &str =
    include_str!("../../sql/queries/eval_runs_later_finding.sql");
pub const EVAL_FINDING_NEXT: &str = include_str!("../../sql/queries/eval_finding_next.sql");
pub const EVAL_FINDINGS_ALL: &str = include_str!("../../sql/queries/eval_findings_all.sql");
pub const EVAL_PASSES: &str = include_str!("../../sql/queries/eval_passes.sql");
pub const EVAL_LATE_TASKS: &str = include_str!("../../sql/queries/eval_late_tasks.sql");
pub const EVAL_ARCHIVE_NEXT: &str = include_str!("../../sql/queries/eval_archive_next.sql");
pub const EVAL_ARCHIVES_ALL: &str = include_str!("../../sql/queries/eval_archives_all.sql");
pub const EVAL_BENCH_NEXT: &str = include_str!("../../sql/queries/eval_bench_next.sql");
pub const EVAL_BENCH_RUNNING: &str = include_str!("../../sql/queries/eval_bench_running.sql");
pub const EVAL_BENCH_FINISH: &str = include_str!("../../sql/queries/eval_bench_finish.sql");
pub const EVAL_TASK_COST: &str = include_str!("../../sql/queries/eval_task_cost.sql");
pub const EVAL_BENCH_ALL: &str = include_str!("../../sql/queries/eval_bench_all.sql");

// A page of the vault's audit log (vault/audit.rs).
pub const VAULT_AUDIT_PAGE: &str = include_str!("../../sql/queries/vault_audit_page.sql");

// The metrics store (metrics/sqlite.rs): samples, windows, events, model calls, kept traces, and the size of
// each table (`metric_size`).
pub const METRIC_AGG_1H_BUCKET_CLEAR: &str =
    include_str!("../../sql/queries/metric_agg_1h_bucket_clear.sql");
pub const METRIC_AGG_1H_BUCKET_PUT: &str =
    include_str!("../../sql/queries/metric_agg_1h_bucket_put.sql");
pub const METRIC_AGG_1H_BUCKET_RANGE: &str =
    include_str!("../../sql/queries/metric_agg_1h_bucket_range.sql");
pub const METRIC_AGG_1H_PRUNE: &str = include_str!("../../sql/queries/metric_agg_1h_prune.sql");
pub const METRIC_AGG_1H_PUT: &str = include_str!("../../sql/queries/metric_agg_1h_put.sql");
pub const METRIC_AGG_1H_RANGE: &str = include_str!("../../sql/queries/metric_agg_1h_range.sql");
pub const METRIC_AGG_1M_BUCKET_CLEAR: &str =
    include_str!("../../sql/queries/metric_agg_1m_bucket_clear.sql");
pub const METRIC_AGG_1M_BUCKET_PUT: &str =
    include_str!("../../sql/queries/metric_agg_1m_bucket_put.sql");
pub const METRIC_AGG_1M_BUCKET_RANGE: &str =
    include_str!("../../sql/queries/metric_agg_1m_bucket_range.sql");
pub const METRIC_AGG_1M_PRUNE: &str = include_str!("../../sql/queries/metric_agg_1m_prune.sql");
pub const METRIC_AGG_1M_PUT: &str = include_str!("../../sql/queries/metric_agg_1m_put.sql");
pub const METRIC_AGG_1M_RANGE: &str = include_str!("../../sql/queries/metric_agg_1m_range.sql");
pub const METRIC_CALL_CONTEXT: &str = include_str!("../../sql/queries/metric_call_context.sql");
pub const METRIC_CALL_CONTEXT_PUT: &str =
    include_str!("../../sql/queries/metric_call_context_put.sql");
pub const METRIC_CALL_PAGE: &str = include_str!("../../sql/queries/metric_call_page.sql");
pub const METRIC_CALL_PRUNE: &str = include_str!("../../sql/queries/metric_call_prune.sql");
pub const METRIC_CALL_PUT: &str = include_str!("../../sql/queries/metric_call_put.sql");
pub const METRIC_EVENT_PAGE: &str = include_str!("../../sql/queries/metric_event_page.sql");
pub const METRIC_EVENT_PART_PUT: &str = include_str!("../../sql/queries/metric_event_part_put.sql");
pub const METRIC_EVENT_PARTS: &str = include_str!("../../sql/queries/metric_event_parts.sql");
pub const METRIC_EVENT_PRUNE: &str = include_str!("../../sql/queries/metric_event_prune.sql");
pub const METRIC_EVENT_PUT: &str = include_str!("../../sql/queries/metric_event_put.sql");
pub const METRIC_SAMPLE_PRUNE: &str = include_str!("../../sql/queries/metric_sample_prune.sql");
pub const METRIC_SAMPLE_PUT: &str = include_str!("../../sql/queries/metric_sample_put.sql");
pub const METRIC_SAMPLE_RANGE: &str = include_str!("../../sql/queries/metric_sample_range.sql");
pub const METRIC_SIZE: &str = include_str!("../../sql/queries/metric_size.sql");
pub const METRIC_TRACE_ATTR_PUT: &str = include_str!("../../sql/queries/metric_trace_attr_put.sql");
pub const METRIC_TRACE_ATTRS: &str = include_str!("../../sql/queries/metric_trace_attrs.sql");
pub const METRIC_TRACE_BY_ID: &str = include_str!("../../sql/queries/metric_trace_by_id.sql");
pub const METRIC_TRACE_BYTES: &str = include_str!("../../sql/queries/metric_trace_bytes.sql");
pub const METRIC_TRACE_DELETE: &str = include_str!("../../sql/queries/metric_trace_delete.sql");
pub const METRIC_TRACE_OLDEST: &str = include_str!("../../sql/queries/metric_trace_oldest.sql");
pub const METRIC_TRACE_PAGE: &str = include_str!("../../sql/queries/metric_trace_page.sql");
pub const METRIC_TRACE_PRUNE: &str = include_str!("../../sql/queries/metric_trace_prune.sql");
pub const METRIC_TRACE_PUT: &str = include_str!("../../sql/queries/metric_trace_put.sql");
pub const METRIC_TRACE_SPAN_PUT: &str = include_str!("../../sql/queries/metric_trace_span_put.sql");
pub const METRIC_TRACE_SPANS: &str = include_str!("../../sql/queries/metric_trace_spans.sql");

// The IDE audit (`ide_audit`: ide.rs writes each owner action, the audit route reads them).
pub const IDE_AUDIT_LAST_SEQ: &str = include_str!("../../sql/queries/ide_audit_last_seq.sql");
pub const IDE_AUDIT_PAGE: &str = include_str!("../../sql/queries/ide_audit_page.sql");
pub const IDE_AUDIT_PAGE_BY_WORKSPACE: &str =
    include_str!("../../sql/queries/ide_audit_page_by_workspace.sql");

// The station audit (`station_audit`: stations/audit.rs).
pub const STATION_AUDIT_LAST_SEQ: &str =
    include_str!("../../sql/queries/station_audit_last_seq.sql");
pub const STATION_AUDIT_PAGE: &str = include_str!("../../sql/queries/station_audit_page.sql");
pub const STATION_AUDIT_PAGE_BY_STATION: &str =
    include_str!("../../sql/queries/station_audit_page_by_station.sql");

// The workstation recordings (`station_recording`, `station_recording_event` and
// `station_recording_frame`: stations/recordings.rs).
pub const STATION_RECORDING_GET: &str = include_str!("../../sql/queries/station_recording_get.sql");
pub const STATION_RECORDING_PAGE: &str =
    include_str!("../../sql/queries/station_recording_page.sql");
pub const STATION_RECORDING_EXPIRED: &str =
    include_str!("../../sql/queries/station_recording_expired.sql");
pub const STATION_RECORDING_DELETE: &str =
    include_str!("../../sql/queries/station_recording_delete.sql");
pub const STATION_RECORDING_EVENT_LAST_SEQ: &str =
    include_str!("../../sql/queries/station_recording_event_last_seq.sql");
pub const STATION_RECORDING_EVENT_PAGE: &str =
    include_str!("../../sql/queries/station_recording_event_page.sql");
pub const STATION_RECORDING_EVENT_DELETE: &str =
    include_str!("../../sql/queries/station_recording_event_delete.sql");
pub const STATION_RECORDING_FRAME_PAGE: &str =
    include_str!("../../sql/queries/station_recording_frame_page.sql");
pub const STATION_RECORDING_FRAME_DELETE: &str =
    include_str!("../../sql/queries/station_recording_frame_delete.sql");
pub const STATION_RECORDING_FRAME_GET: &str =
    include_str!("../../sql/queries/station_recording_frame_get.sql");
pub const STATION_RECORDING_FRAME_HASHES: &str =
    include_str!("../../sql/queries/station_recording_frame_hashes.sql");

// The work-folder sync's mirror (`station_sync_file`: stations/sync.rs).
pub const STATION_SYNC_FILE_ROWS: &str =
    include_str!("../../sql/queries/station_sync_file_rows.sql");
pub const STATION_SYNC_CONTENT_HASHES: &str =
    include_str!("../../sql/queries/station_sync_content_hashes.sql");

// The transcripts (`entry` in transcripts.db; transcript.rs).
pub const TRANSCRIPT_PAGE: &str = include_str!("../../sql/queries/transcript_page.sql");
pub const TRANSCRIPT_CIDS: &str = include_str!("../../sql/queries/transcript_cids.sql");
pub const TRANSCRIPT_DELETE: &str = include_str!("../../sql/queries/transcript_delete.sql");
pub const TRANSCRIPT_BEFORE_SESSION: &str =
    include_str!("../../sql/queries/transcript_before_session.sql");
pub const TRANSCRIPT_BEFORE_THREAD: &str =
    include_str!("../../sql/queries/transcript_before_thread.sql");

// The unfurl cache (`unfurl_cache`: unfurl/mod.rs).
pub const UNFURL_GET: &str = include_str!("../../sql/queries/unfurl_get.sql");

// The hub MCP server (`hub_mcp_token`, `hub_mcp_token_digest` and `hub_mcp_call`: hub_mcp/store_db.rs).
pub const HUB_MCP_CALL_LAST_SEQ: &str = include_str!("../../sql/queries/hub_mcp_call_last_seq.sql");
pub const HUB_MCP_CALL_PAGE: &str = include_str!("../../sql/queries/hub_mcp_call_page.sql");
pub const HUB_MCP_TOKEN_BY_DIGEST: &str =
    include_str!("../../sql/queries/hub_mcp_token_by_digest.sql");
pub const HUB_MCP_TOKEN_NEXT_POS: &str =
    include_str!("../../sql/queries/hub_mcp_token_next_pos.sql");
pub const HUB_MCP_TOKEN_REVOKE: &str = include_str!("../../sql/queries/hub_mcp_token_revoke.sql");
pub const HUB_MCP_TOKEN_USE: &str = include_str!("../../sql/queries/hub_mcp_token_use.sql");

// The prompt library (`prompt`, `prompt_version` and `prompt_event`: prompts/store.rs).
pub const PROMPT_LIST: &str = include_str!("../../sql/queries/prompt_list.sql");
pub const PROMPT_GET: &str = include_str!("../../sql/queries/prompt_get.sql");
pub const PROMPT_TAGS: &str = include_str!("../../sql/queries/prompt_tags.sql");
pub const PROMPT_TAGS_CLEAR: &str = include_str!("../../sql/queries/prompt_tags_clear.sql");
pub const PROMPT_POS: &str = include_str!("../../sql/queries/prompt_pos.sql");
pub const PROMPT_BINDING_HOLDER: &str = include_str!("../../sql/queries/prompt_binding_holder.sql");
pub const PROMPT_BOUND: &str = include_str!("../../sql/queries/prompt_bound.sql");
pub const PROMPT_VERSIONS: &str = include_str!("../../sql/queries/prompt_versions.sql");
pub const PROMPT_VERSION_GET: &str = include_str!("../../sql/queries/prompt_version_get.sql");
pub const PROMPT_FINDINGS: &str = include_str!("../../sql/queries/prompt_findings.sql");
pub const PROMPT_BODY: &str = include_str!("../../sql/queries/prompt_body.sql");
pub const PROMPT_EVENTS: &str = include_str!("../../sql/queries/prompt_events.sql");
pub const PROMPT_EVENT_LAST_SEQ: &str = include_str!("../../sql/queries/prompt_event_last_seq.sql");

// The skill library (`skill`, `skill_version`, `skill_file`, `skill_finding` and `skill_event`: skills/store_db.rs).
pub const SKILL_LIST: &str = include_str!("../../sql/queries/skill_list.sql");
pub const SKILL_GET: &str = include_str!("../../sql/queries/skill_get.sql");
pub const SKILL_POS: &str = include_str!("../../sql/queries/skill_pos.sql");
pub const SKILL_TAGS: &str = include_str!("../../sql/queries/skill_tags.sql");
pub const SKILL_TAGS_CLEAR: &str = include_str!("../../sql/queries/skill_tags_clear.sql");
pub const SKILL_BINDINGS: &str = include_str!("../../sql/queries/skill_bindings.sql");
pub const SKILL_BINDINGS_CLEAR: &str = include_str!("../../sql/queries/skill_bindings_clear.sql");
pub const SKILL_NAME_HOLDER: &str = include_str!("../../sql/queries/skill_name_holder.sql");
pub const SKILL_VERSIONS: &str = include_str!("../../sql/queries/skill_versions.sql");
pub const SKILL_VERSION_META: &str = include_str!("../../sql/queries/skill_version_meta.sql");
pub const SKILL_VERSION_GET: &str = include_str!("../../sql/queries/skill_version_get.sql");
pub const SKILL_FILES: &str = include_str!("../../sql/queries/skill_files.sql");
pub const SKILL_FINDINGS: &str = include_str!("../../sql/queries/skill_findings.sql");
pub const SKILL_EVENTS: &str = include_str!("../../sql/queries/skill_events.sql");
pub const SKILL_EVENT_LAST_SEQ: &str = include_str!("../../sql/queries/skill_event_last_seq.sql");
pub const SKILL_BOUND: &str = include_str!("../../sql/queries/skill_bound.sql");

// The previews (`preview`, `preview_link` and `preview_opening`: previews/mod.rs).
pub const PREVIEW_POS: &str = include_str!("../../sql/queries/preview_pos.sql");
pub const PREVIEW_GET: &str = include_str!("../../sql/queries/preview_get.sql");
pub const PREVIEW_LIST: &str = include_str!("../../sql/queries/preview_list.sql");
pub const PREVIEW_LIST_PROJECT: &str = include_str!("../../sql/queries/preview_list_project.sql");
pub const PREVIEW_LIST_LANE: &str = include_str!("../../sql/queries/preview_list_lane.sql");
pub const PREVIEW_OPEN: &str = include_str!("../../sql/queries/preview_open.sql");
pub const LINK_POS: &str = include_str!("../../sql/queries/preview_link_pos.sql");
pub const LINK_GET: &str = include_str!("../../sql/queries/preview_link_get.sql");
pub const LINK_LIST_PREVIEW: &str = include_str!("../../sql/queries/preview_link_list.sql");
pub const PREVIEW_OPENING_LAST_SEQ: &str =
    include_str!("../../sql/queries/preview_opening_last_seq.sql");
pub const PREVIEW_OPENING_PAGE: &str = include_str!("../../sql/queries/preview_opening_page.sql");

// The share links (`share` and `share_event`: shares/mod.rs).
pub const SHARE_GET: &str = include_str!("../../sql/queries/share_get.sql");
pub const SHARE_LIST: &str = include_str!("../../sql/queries/share_list.sql");
pub const SHARE_LIST_TARGET: &str = include_str!("../../sql/queries/share_list_target.sql");
pub const SHARE_LIVE: &str = include_str!("../../sql/queries/share_live.sql");
pub const SHARE_POS: &str = include_str!("../../sql/queries/share_pos.sql");
pub const SHARE_EVENT_COUNTS: &str = include_str!("../../sql/queries/share_event_counts.sql");
pub const SHARE_EVENT_FAILURES: &str = include_str!("../../sql/queries/share_event_failures.sql");
pub const SHARE_EVENT_LAST_SEQ: &str = include_str!("../../sql/queries/share_event_last_seq.sql");
pub const SHARE_EVENT_PAGE: &str = include_str!("../../sql/queries/share_event_page.sql");

// The artifact index (`artifact`, `artifact_version` and `artifact_blob`: artifacts/mod.rs).
pub const ARTIFACT_GET: &str = include_str!("../../sql/queries/artifact_get.sql");
pub const ARTIFACT_POS: &str = include_str!("../../sql/queries/artifact_pos.sql");
pub const ARTIFACT_LIST: &str = include_str!("../../sql/queries/artifact_list.sql");
pub const ARTIFACT_LATEST: &str = include_str!("../../sql/queries/artifact_latest.sql");
pub const ARTIFACT_SET_SHARED: &str = include_str!("../../sql/queries/artifact_set_shared.sql");
pub const ARTIFACT_DELETE: &str = include_str!("../../sql/queries/artifact_delete.sql");
pub const ARTIFACT_HASHES: &str = include_str!("../../sql/queries/artifact_hashes.sql");
pub const ARTIFACT_STATS_VERSIONS: &str =
    include_str!("../../sql/queries/artifact_stats_versions.sql");
pub const ARTIFACT_VERSION_GET: &str = include_str!("../../sql/queries/artifact_version_get.sql");
pub const ARTIFACT_VERSION_LIST: &str = include_str!("../../sql/queries/artifact_version_list.sql");
pub const ARTIFACT_VERSION_BLOBS: &str =
    include_str!("../../sql/queries/artifact_version_blobs.sql");
pub const ARTIFACT_VERSION_SCRIPTS: &str =
    include_str!("../../sql/queries/artifact_version_scripts.sql");
pub const ARTIFACT_SCRIPT_HASH: &str = include_str!("../../sql/queries/artifact_script_hash.sql");
pub const ARTIFACT_VERSION_DELETE: &str =
    include_str!("../../sql/queries/artifact_version_delete.sql");
pub const ARTIFACT_BLOB_GET: &str = include_str!("../../sql/queries/artifact_blob_get.sql");
pub const ARTIFACT_BLOB_POS: &str = include_str!("../../sql/queries/artifact_blob_pos.sql");
pub const ARTIFACT_STATS_BLOBS: &str = include_str!("../../sql/queries/artifact_stats_blobs.sql");
pub const ARTIFACT_BLOB_IDS: &str = include_str!("../../sql/queries/artifact_blob_ids.sql");
pub const ARTIFACT_BLOB_USERS: &str = include_str!("../../sql/queries/artifact_blob_users.sql");
pub const ARTIFACT_REFERS: &str = include_str!("../../sql/queries/artifact_refers.sql");
pub const ARTIFACT_BLOB_DELETE: &str = include_str!("../../sql/queries/artifact_blob_delete.sql");

// The full-text indexes of the journal's notes and the project memory's entries (store/search.rs): rebuilt by the
// saver, and searched.
pub const JOURNAL_SEARCH_CLEAR: &str = include_str!("../../sql/queries/journal_search_clear.sql");
pub const JOURNAL_SEARCH_FILL: &str = include_str!("../../sql/queries/journal_search_fill.sql");
pub const JOURNAL_SEARCH_HITS: &str = include_str!("../../sql/queries/journal_search_hits.sql");
pub const MEMORY_SEARCH_CLEAR: &str = include_str!("../../sql/queries/memory_search_clear.sql");
pub const MEMORY_SEARCH_FILL: &str = include_str!("../../sql/queries/memory_search_fill.sql");
pub const MEMORY_SEARCH_HITS: &str = include_str!("../../sql/queries/memory_search_hits.sql");

/// Every query: (file, name, text), for the test that prepares each against its file.
#[cfg(test)]
pub const QUERIES: &[(&str, &str, &str)] = &[
    ("plaza", "preview_pos", PREVIEW_POS),
    ("plaza", "preview_get", PREVIEW_GET),
    ("plaza", "preview_list", PREVIEW_LIST),
    ("plaza", "preview_list_project", PREVIEW_LIST_PROJECT),
    ("plaza", "preview_list_lane", PREVIEW_LIST_LANE),
    ("plaza", "preview_open", PREVIEW_OPEN),
    ("plaza", "preview_link_pos", LINK_POS),
    ("plaza", "preview_link_get", LINK_GET),
    ("plaza", "preview_link_list", LINK_LIST_PREVIEW),
    (
        "plaza",
        "preview_opening_last_seq",
        PREVIEW_OPENING_LAST_SEQ,
    ),
    ("plaza", "preview_opening_page", PREVIEW_OPENING_PAGE),
    ("plaza", "budget_ledger", BUDGET_LEDGER),
    ("plaza", "usage_calls_clear", USAGE_CALLS_CLEAR),
    ("plaza", "usage_calls_count", USAGE_CALLS_COUNT),
    ("plaza", "usage_calls_page", USAGE_CALLS_PAGE),
    ("plaza", "usage_calls_prune", USAGE_CALLS_PRUNE),
    ("plaza", "usage_escalated", USAGE_ESCALATED),
    ("plaza", "usage_hour_add", USAGE_HOUR_ADD),
    ("plaza", "usage_hours_prune", USAGE_HOURS_PRUNE),
    ("plaza", "usage_newest_id", USAGE_NEWEST_ID),
    ("plaza", "usage_series", USAGE_SERIES),
    ("plaza", "usage_summary", USAGE_SUMMARY),
    ("plaza", "usage_work_add", USAGE_WORK_ADD),
    ("plaza", "usage_work_totals", USAGE_WORK_TOTALS),
    ("plaza", "usage_works_prune", USAGE_WORKS_PRUNE),
    ("plaza", "eval_runs_page", EVAL_RUNS_PAGE),
    ("plaza", "eval_scores", EVAL_SCORES),
    ("plaza", "eval_runs_landed", EVAL_RUNS_LANDED),
    ("plaza", "eval_runs_all", EVAL_RUNS_ALL),
    ("plaza", "eval_runs_reverted", EVAL_RUNS_REVERTED),
    ("plaza", "eval_runs_incident", EVAL_RUNS_INCIDENT),
    ("plaza", "eval_runs_qualified", EVAL_RUNS_QUALIFIED),
    ("plaza", "eval_runs_later_finding", EVAL_RUNS_LATER_FINDING),
    ("plaza", "eval_finding_next", EVAL_FINDING_NEXT),
    ("plaza", "eval_findings_all", EVAL_FINDINGS_ALL),
    ("plaza", "eval_passes", EVAL_PASSES),
    ("plaza", "eval_late_tasks", EVAL_LATE_TASKS),
    ("plaza", "eval_archive_next", EVAL_ARCHIVE_NEXT),
    ("plaza", "eval_archives_all", EVAL_ARCHIVES_ALL),
    ("plaza", "eval_bench_all", EVAL_BENCH_ALL),
    ("plaza", "eval_bench_next", EVAL_BENCH_NEXT),
    ("plaza", "eval_bench_running", EVAL_BENCH_RUNNING),
    ("plaza", "eval_bench_finish", EVAL_BENCH_FINISH),
    ("plaza", "eval_task_cost", EVAL_TASK_COST),
    ("plaza", "vault_audit_page", VAULT_AUDIT_PAGE),
    ("plaza", "hub_mcp_call_last_seq", HUB_MCP_CALL_LAST_SEQ),
    ("plaza", "hub_mcp_call_page", HUB_MCP_CALL_PAGE),
    ("plaza", "hub_mcp_token_by_digest", HUB_MCP_TOKEN_BY_DIGEST),
    ("plaza", "hub_mcp_token_next_pos", HUB_MCP_TOKEN_NEXT_POS),
    ("plaza", "hub_mcp_token_revoke", HUB_MCP_TOKEN_REVOKE),
    ("plaza", "hub_mcp_token_use", HUB_MCP_TOKEN_USE),
    ("plaza", "unfurl_get", UNFURL_GET),
    ("plaza", "ide_audit_last_seq", IDE_AUDIT_LAST_SEQ),
    ("plaza", "ide_audit_page", IDE_AUDIT_PAGE),
    (
        "plaza",
        "ide_audit_page_by_workspace",
        IDE_AUDIT_PAGE_BY_WORKSPACE,
    ),
    ("plaza", "station_audit_last_seq", STATION_AUDIT_LAST_SEQ),
    ("plaza", "station_audit_page", STATION_AUDIT_PAGE),
    (
        "plaza",
        "station_audit_page_by_station",
        STATION_AUDIT_PAGE_BY_STATION,
    ),
    ("plaza", "station_recording_get", STATION_RECORDING_GET),
    ("plaza", "station_recording_page", STATION_RECORDING_PAGE),
    (
        "plaza",
        "station_recording_expired",
        STATION_RECORDING_EXPIRED,
    ),
    (
        "plaza",
        "station_recording_delete",
        STATION_RECORDING_DELETE,
    ),
    (
        "plaza",
        "station_recording_event_last_seq",
        STATION_RECORDING_EVENT_LAST_SEQ,
    ),
    (
        "plaza",
        "station_recording_event_page",
        STATION_RECORDING_EVENT_PAGE,
    ),
    (
        "plaza",
        "station_recording_event_delete",
        STATION_RECORDING_EVENT_DELETE,
    ),
    (
        "plaza",
        "station_recording_frame_page",
        STATION_RECORDING_FRAME_PAGE,
    ),
    (
        "plaza",
        "station_recording_frame_delete",
        STATION_RECORDING_FRAME_DELETE,
    ),
    (
        "plaza",
        "station_recording_frame_get",
        STATION_RECORDING_FRAME_GET,
    ),
    (
        "plaza",
        "station_recording_frame_hashes",
        STATION_RECORDING_FRAME_HASHES,
    ),
    ("plaza", "station_sync_file_rows", STATION_SYNC_FILE_ROWS),
    (
        "plaza",
        "station_sync_content_hashes",
        STATION_SYNC_CONTENT_HASHES,
    ),
    (
        "metrics",
        "metric_agg_1h_bucket_clear",
        METRIC_AGG_1H_BUCKET_CLEAR,
    ),
    (
        "metrics",
        "metric_agg_1h_bucket_put",
        METRIC_AGG_1H_BUCKET_PUT,
    ),
    (
        "metrics",
        "metric_agg_1h_bucket_range",
        METRIC_AGG_1H_BUCKET_RANGE,
    ),
    ("metrics", "metric_agg_1h_prune", METRIC_AGG_1H_PRUNE),
    ("metrics", "metric_agg_1h_put", METRIC_AGG_1H_PUT),
    ("metrics", "metric_agg_1h_range", METRIC_AGG_1H_RANGE),
    (
        "metrics",
        "metric_agg_1m_bucket_clear",
        METRIC_AGG_1M_BUCKET_CLEAR,
    ),
    (
        "metrics",
        "metric_agg_1m_bucket_put",
        METRIC_AGG_1M_BUCKET_PUT,
    ),
    (
        "metrics",
        "metric_agg_1m_bucket_range",
        METRIC_AGG_1M_BUCKET_RANGE,
    ),
    ("metrics", "metric_agg_1m_prune", METRIC_AGG_1M_PRUNE),
    ("metrics", "metric_agg_1m_put", METRIC_AGG_1M_PUT),
    ("metrics", "metric_agg_1m_range", METRIC_AGG_1M_RANGE),
    ("metrics", "metric_call_context", METRIC_CALL_CONTEXT),
    (
        "metrics",
        "metric_call_context_put",
        METRIC_CALL_CONTEXT_PUT,
    ),
    ("metrics", "metric_call_page", METRIC_CALL_PAGE),
    ("metrics", "metric_call_prune", METRIC_CALL_PRUNE),
    ("metrics", "metric_call_put", METRIC_CALL_PUT),
    ("metrics", "metric_event_page", METRIC_EVENT_PAGE),
    ("metrics", "metric_event_part_put", METRIC_EVENT_PART_PUT),
    ("metrics", "metric_event_parts", METRIC_EVENT_PARTS),
    ("metrics", "metric_event_prune", METRIC_EVENT_PRUNE),
    ("metrics", "metric_event_put", METRIC_EVENT_PUT),
    ("metrics", "metric_sample_prune", METRIC_SAMPLE_PRUNE),
    ("metrics", "metric_sample_put", METRIC_SAMPLE_PUT),
    ("metrics", "metric_sample_range", METRIC_SAMPLE_RANGE),
    ("metrics", "metric_size", METRIC_SIZE),
    ("metrics", "metric_trace_attr_put", METRIC_TRACE_ATTR_PUT),
    ("metrics", "metric_trace_attrs", METRIC_TRACE_ATTRS),
    ("metrics", "metric_trace_by_id", METRIC_TRACE_BY_ID),
    ("metrics", "metric_trace_bytes", METRIC_TRACE_BYTES),
    ("metrics", "metric_trace_delete", METRIC_TRACE_DELETE),
    ("metrics", "metric_trace_oldest", METRIC_TRACE_OLDEST),
    ("metrics", "metric_trace_page", METRIC_TRACE_PAGE),
    ("metrics", "metric_trace_prune", METRIC_TRACE_PRUNE),
    ("metrics", "metric_trace_put", METRIC_TRACE_PUT),
    ("metrics", "metric_trace_span_put", METRIC_TRACE_SPAN_PUT),
    ("metrics", "metric_trace_spans", METRIC_TRACE_SPANS),
    ("transcripts", "transcript_page", TRANSCRIPT_PAGE),
    ("transcripts", "transcript_cids", TRANSCRIPT_CIDS),
    ("transcripts", "transcript_delete", TRANSCRIPT_DELETE),
    (
        "transcripts",
        "transcript_before_session",
        TRANSCRIPT_BEFORE_SESSION,
    ),
    (
        "transcripts",
        "transcript_before_thread",
        TRANSCRIPT_BEFORE_THREAD,
    ),
    ("plaza", "prompt_list", PROMPT_LIST),
    ("plaza", "prompt_get", PROMPT_GET),
    ("plaza", "prompt_tags", PROMPT_TAGS),
    ("plaza", "prompt_tags_clear", PROMPT_TAGS_CLEAR),
    ("plaza", "prompt_pos", PROMPT_POS),
    ("plaza", "prompt_binding_holder", PROMPT_BINDING_HOLDER),
    ("plaza", "prompt_bound", PROMPT_BOUND),
    ("plaza", "prompt_versions", PROMPT_VERSIONS),
    ("plaza", "prompt_version_get", PROMPT_VERSION_GET),
    ("plaza", "prompt_findings", PROMPT_FINDINGS),
    ("plaza", "prompt_body", PROMPT_BODY),
    ("plaza", "prompt_events", PROMPT_EVENTS),
    ("plaza", "prompt_event_last_seq", PROMPT_EVENT_LAST_SEQ),
    ("plaza", "skill_list", SKILL_LIST),
    ("plaza", "skill_get", SKILL_GET),
    ("plaza", "skill_pos", SKILL_POS),
    ("plaza", "skill_tags", SKILL_TAGS),
    ("plaza", "skill_tags_clear", SKILL_TAGS_CLEAR),
    ("plaza", "skill_bindings", SKILL_BINDINGS),
    ("plaza", "skill_bindings_clear", SKILL_BINDINGS_CLEAR),
    ("plaza", "skill_name_holder", SKILL_NAME_HOLDER),
    ("plaza", "skill_versions", SKILL_VERSIONS),
    ("plaza", "skill_version_meta", SKILL_VERSION_META),
    ("plaza", "skill_version_get", SKILL_VERSION_GET),
    ("plaza", "skill_files", SKILL_FILES),
    ("plaza", "skill_findings", SKILL_FINDINGS),
    ("plaza", "skill_events", SKILL_EVENTS),
    ("plaza", "skill_event_last_seq", SKILL_EVENT_LAST_SEQ),
    ("plaza", "skill_bound", SKILL_BOUND),
    ("plaza", "share_get", SHARE_GET),
    ("plaza", "share_list", SHARE_LIST),
    ("plaza", "share_list_target", SHARE_LIST_TARGET),
    ("plaza", "share_live", SHARE_LIVE),
    ("plaza", "share_pos", SHARE_POS),
    ("plaza", "share_event_counts", SHARE_EVENT_COUNTS),
    ("plaza", "share_event_failures", SHARE_EVENT_FAILURES),
    ("plaza", "share_event_last_seq", SHARE_EVENT_LAST_SEQ),
    ("plaza", "share_event_page", SHARE_EVENT_PAGE),
    ("plaza", "artifact_get", ARTIFACT_GET),
    ("plaza", "artifact_pos", ARTIFACT_POS),
    ("plaza", "artifact_list", ARTIFACT_LIST),
    ("plaza", "artifact_latest", ARTIFACT_LATEST),
    ("plaza", "artifact_set_shared", ARTIFACT_SET_SHARED),
    ("plaza", "artifact_delete", ARTIFACT_DELETE),
    ("plaza", "artifact_hashes", ARTIFACT_HASHES),
    ("plaza", "artifact_stats_versions", ARTIFACT_STATS_VERSIONS),
    ("plaza", "artifact_version_get", ARTIFACT_VERSION_GET),
    ("plaza", "artifact_version_list", ARTIFACT_VERSION_LIST),
    ("plaza", "artifact_version_blobs", ARTIFACT_VERSION_BLOBS),
    (
        "plaza",
        "artifact_version_scripts",
        ARTIFACT_VERSION_SCRIPTS,
    ),
    ("plaza", "artifact_script_hash", ARTIFACT_SCRIPT_HASH),
    ("plaza", "artifact_version_delete", ARTIFACT_VERSION_DELETE),
    ("plaza", "artifact_blob_get", ARTIFACT_BLOB_GET),
    ("plaza", "artifact_blob_pos", ARTIFACT_BLOB_POS),
    ("plaza", "artifact_stats_blobs", ARTIFACT_STATS_BLOBS),
    ("plaza", "artifact_blob_ids", ARTIFACT_BLOB_IDS),
    ("plaza", "artifact_blob_users", ARTIFACT_BLOB_USERS),
    ("plaza", "artifact_refers", ARTIFACT_REFERS),
    ("plaza", "artifact_blob_delete", ARTIFACT_BLOB_DELETE),
    ("plaza", "journal_search_clear", JOURNAL_SEARCH_CLEAR),
    ("plaza", "journal_search_fill", JOURNAL_SEARCH_FILL),
    ("plaza", "journal_search_hits", JOURNAL_SEARCH_HITS),
    ("plaza", "memory_search_clear", MEMORY_SEARCH_CLEAR),
    ("plaza", "memory_search_fill", MEMORY_SEARCH_FILL),
    ("plaza", "memory_search_hits", MEMORY_SEARCH_HITS),
];
