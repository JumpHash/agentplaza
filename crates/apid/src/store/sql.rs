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

// The transcripts (`entry` in transcripts.db; transcript.rs).
pub const TRANSCRIPT_PAGE: &str = include_str!("../../sql/queries/transcript_page.sql");
pub const TRANSCRIPT_CIDS: &str = include_str!("../../sql/queries/transcript_cids.sql");
pub const TRANSCRIPT_DELETE: &str = include_str!("../../sql/queries/transcript_delete.sql");

/// Every query: (file, name, text), for the test that prepares each against its file.
#[cfg(test)]
pub const QUERIES: &[(&str, &str, &str)] = &[
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
    ("plaza", "vault_audit_page", VAULT_AUDIT_PAGE),
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
];
