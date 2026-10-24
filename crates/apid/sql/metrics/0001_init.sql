-- metrics.db, schema 1 (the clean break, W3): metric samples and their rollups. STRICT, typed, no JSON (W69).
-- Streams are written in batches by the hub's flush and read through the file's readers; retention deletes by age.

-- metric (samples): one row per 10-second point of a series. The series id is metric|kind|entity|labels; the metric
-- is kept too, so a read of one metric's points in a time range is one index range.
CREATE TABLE metric_sample (
    metric TEXT NOT NULL,
    t_ms INTEGER NOT NULL,
    series TEXT NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY (metric, t_ms, series)
) STRICT, WITHOUT ROWID;
CREATE INDEX metric_sample_t ON metric_sample (t_ms);

-- metric (windows): one row per closed 1-minute window of a series; a histogram's buckets are child rows.
CREATE TABLE metric_agg_1m (
    metric TEXT NOT NULL,
    window_ms INTEGER NOT NULL,
    series TEXT NOT NULL,
    count INTEGER NOT NULL,
    sum REAL NOT NULL,
    min REAL NOT NULL,
    max REAL NOT NULL,
    last REAL NOT NULL,
    hist_count INTEGER,
    hist_sum REAL,
    hist_min REAL,
    hist_max REAL,
    PRIMARY KEY (metric, window_ms, series)
) STRICT, WITHOUT ROWID;
CREATE INDEX metric_agg_1m_window ON metric_agg_1m (window_ms);

CREATE TABLE metric_agg_1m_bucket (
    metric TEXT NOT NULL,
    window_ms INTEGER NOT NULL,
    series TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (metric, window_ms, series, bucket),
    FOREIGN KEY (metric, window_ms, series) REFERENCES metric_agg_1m (metric, window_ms, series) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- metric (windows): one row per closed 1-hour window of a series; a histogram's buckets are child rows.
CREATE TABLE metric_agg_1h (
    metric TEXT NOT NULL,
    window_ms INTEGER NOT NULL,
    series TEXT NOT NULL,
    count INTEGER NOT NULL,
    sum REAL NOT NULL,
    min REAL NOT NULL,
    max REAL NOT NULL,
    last REAL NOT NULL,
    hist_count INTEGER,
    hist_sum REAL,
    hist_min REAL,
    hist_max REAL,
    PRIMARY KEY (metric, window_ms, series)
) STRICT, WITHOUT ROWID;
CREATE INDEX metric_agg_1h_window ON metric_agg_1h (window_ms);

CREATE TABLE metric_agg_1h_bucket (
    metric TEXT NOT NULL,
    window_ms INTEGER NOT NULL,
    series TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (metric, window_ms, series, bucket),
    FOREIGN KEY (metric, window_ms, series) REFERENCES metric_agg_1h (metric, window_ms, series) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- events: something that happened, with how long it took when that is the point of it.
CREATE TABLE metric_event (
    id INTEGER PRIMARY KEY,
    t_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    entity_kind TEXT NOT NULL CHECK (entity_kind IN ('global', 'node', 'run', 'vm', 'workstation', 'sandbox', 'agent',
        'role', 'provider', 'model', 'tool', 'route', 'service', 'volume', 'engine', 'hub')),
    entity TEXT NOT NULL,
    duration_ms REAL,
    detail TEXT NOT NULL
) STRICT;
CREATE INDEX metric_event_t ON metric_event (t_ms);

CREATE TABLE metric_event_part (
    event INTEGER NOT NULL REFERENCES metric_event (id) ON DELETE CASCADE,
    pos INTEGER NOT NULL,
    name TEXT NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY (event, pos)
) STRICT, WITHOUT ROWID;

-- model calls: the usage ledger's call plus its timings and its context split (model_call_context).
CREATE TABLE model_call (
    id INTEGER PRIMARY KEY,
    at_ms INTEGER NOT NULL,
    cid INTEGER NOT NULL,
    role TEXT NOT NULL,
    run TEXT NOT NULL,
    node TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    engine TEXT NOT NULL,
    tokens_in INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    tokens_out INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    ttft_ms REAL,
    duration_ms REAL,
    status INTEGER NOT NULL,
    retries INTEGER NOT NULL,
    task TEXT,
    trace_id TEXT
) STRICT;
CREATE INDEX model_call_at ON model_call (at_ms);

CREATE TABLE model_call_context (
    call INTEGER NOT NULL REFERENCES model_call (id) ON DELETE CASCADE,
    pos INTEGER NOT NULL,
    part TEXT NOT NULL,
    tokens INTEGER NOT NULL,
    PRIMARY KEY (call, pos)
) STRICT, WITHOUT ROWID;

-- traces: one row per kept turn; its spans and their attributes are child rows.
CREATE TABLE trace (
    seq INTEGER PRIMARY KEY,
    id TEXT NOT NULL,
    cid INTEGER NOT NULL,
    role TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    dur_ms REAL NOT NULL,
    ok INTEGER NOT NULL CHECK (ok IN (0, 1)),
    kept TEXT CHECK (kept IN ('error', 'slow', 'open', 'sampled')),
    bytes INTEGER NOT NULL
) STRICT;
CREATE INDEX trace_start ON trace (start_ms);
CREATE INDEX trace_id ON trace (id);

CREATE TABLE trace_span (
    trace INTEGER NOT NULL REFERENCES trace (seq) ON DELETE CASCADE,
    pos INTEGER NOT NULL,
    span_id INTEGER NOT NULL,
    parent INTEGER,
    kind TEXT NOT NULL CHECK (kind IN ('model', 'tool', 'hub_mcp', 'wake', 'restore', 'ready')),
    name TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    dur_ms REAL NOT NULL,
    first_token_ms REAL,
    ok INTEGER NOT NULL CHECK (ok IN (0, 1)),
    PRIMARY KEY (trace, pos)
) STRICT, WITHOUT ROWID;

CREATE TABLE trace_span_attr (
    trace INTEGER NOT NULL,
    span INTEGER NOT NULL,
    pos INTEGER NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (trace, span, pos),
    FOREIGN KEY (trace, span) REFERENCES trace_span (trace, pos) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- saved values: one row each (the settings, the alert state, the dashboards); generated by derive(Table) from their
-- structs (metrics/settings.rs, metrics/alerting.rs, apz-metrics-view model.rs). The test in metrics/sqlite.rs checks them.
CREATE TABLE "metric_settings" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "level" TEXT NOT NULL CHECK ("level" IN ('off', 'basic', 'standard', 'full')),
    "retention_chunks_hours" INTEGER NOT NULL CHECK ("retention_chunks_hours" >= 0),
    "retention_minute_days" INTEGER NOT NULL CHECK ("retention_minute_days" >= 0),
    "retention_hour_days" INTEGER NOT NULL CHECK ("retention_hour_days" >= 0),
    "retention_calls_days" INTEGER NOT NULL CHECK ("retention_calls_days" >= 0),
    "retention_traces_days" INTEGER NOT NULL CHECK ("retention_traces_days" >= 0),
    "retention_traces_cap_mb" INTEGER NOT NULL CHECK ("retention_traces_cap_mb" >= 0),
    "retention_events_days" INTEGER NOT NULL CHECK ("retention_events_days" >= 0),
    "monthly_budget_usd" REAL,
    "basis_days" INTEGER NOT NULL CHECK ("basis_days" >= 0),
    "outside_openmetrics" INTEGER NOT NULL CHECK ("outside_openmetrics" IN (0, 1)),
    "outside_openmetrics_token" TEXT NOT NULL,
    "outside_otlp" INTEGER NOT NULL CHECK ("outside_otlp" IN (0, 1)),
    "outside_otlp_url" TEXT NOT NULL,
    "outside_otlp_confirmed" INTEGER NOT NULL CHECK ("outside_otlp_confirmed" IN (0, 1)),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "metric_settings_budgets" (
    "metric_settings_pos" INTEGER NOT NULL CHECK ("metric_settings_pos" >= 0),
    "budgets_pos" INTEGER NOT NULL CHECK ("budgets_pos" >= 0),
    "scope" TEXT NOT NULL,
    "cap_usd" REAL NOT NULL,
    PRIMARY KEY ("metric_settings_pos", "budgets_pos"),
    FOREIGN KEY ("metric_settings_pos") REFERENCES "metric_settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE TABLE "metric_alerts" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "metric_alerts_rules" (
    "metric_alerts_pos" INTEGER NOT NULL CHECK ("metric_alerts_pos" >= 0),
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "metric" TEXT NOT NULL,
    "agg" TEXT NOT NULL CHECK ("agg" IN ('sum', 'avg', 'min', 'max', 'last', 'rate', 'count', 'p10', 'p50', 'p90', 'p95', 'p99')),
    "scope" TEXT NOT NULL,
    "cmp" TEXT NOT NULL CHECK ("cmp" IN ('gt', 'lt')),
    "threshold" REAL NOT NULL,
    "for_ms" INTEGER NOT NULL CHECK ("for_ms" >= 0),
    "severity" TEXT NOT NULL CHECK ("severity" IN ('warning', 'critical')),
    "on" INTEGER NOT NULL CHECK ("on" IN (0, 1)),
    "notify_console" INTEGER NOT NULL CHECK ("notify_console" IN (0, 1)),
    "notify_desktop" INTEGER NOT NULL CHECK ("notify_desktop" IN (0, 1)),
    "notify_board" INTEGER NOT NULL CHECK ("notify_board" IN (0, 1)),
    "starter" INTEGER NOT NULL CHECK ("starter" IN (0, 1)),
    PRIMARY KEY ("metric_alerts_pos", "rules_pos"),
    FOREIGN KEY ("metric_alerts_pos") REFERENCES "metric_alerts" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "metric_alerts_states" (
    "metric_alerts_pos" INTEGER NOT NULL CHECK ("metric_alerts_pos" >= 0),
    "states_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('ok', 'pending', 'firing', 'clearing')),
    "pending_since_ms" INTEGER CHECK ("pending_since_ms" >= 0),
    "firing_since_ms" INTEGER CHECK ("firing_since_ms" >= 0),
    "clearing_since_ms" INTEGER CHECK ("clearing_since_ms" >= 0),
    "clearing_clear_ms" INTEGER CHECK ("clearing_clear_ms" >= 0),
    PRIMARY KEY ("metric_alerts_pos", "states_key"),
    FOREIGN KEY ("metric_alerts_pos") REFERENCES "metric_alerts" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "metric_alerts_muted" (
    "metric_alerts_pos" INTEGER NOT NULL CHECK ("metric_alerts_pos" >= 0),
    "muted_pos" INTEGER NOT NULL CHECK ("muted_pos" >= 0),
    "rule" TEXT NOT NULL,
    "until_ms" INTEGER NOT NULL CHECK ("until_ms" >= 0),
    PRIMARY KEY ("metric_alerts_pos", "muted_pos"),
    FOREIGN KEY ("metric_alerts_pos") REFERENCES "metric_alerts" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "metric_alerts_acked" (
    "metric_alerts_pos" INTEGER NOT NULL CHECK ("metric_alerts_pos" >= 0),
    "acked_pos" INTEGER NOT NULL CHECK ("acked_pos" >= 0),
    "rule" TEXT NOT NULL,
    "subject" TEXT NOT NULL,
    PRIMARY KEY ("metric_alerts_pos", "acked_pos"),
    FOREIGN KEY ("metric_alerts_pos") REFERENCES "metric_alerts" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "metric_alerts_history" (
    "metric_alerts_pos" INTEGER NOT NULL CHECK ("metric_alerts_pos" >= 0),
    "history_pos" INTEGER NOT NULL CHECK ("history_pos" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "rule" TEXT NOT NULL,
    "subject" TEXT NOT NULL,
    "what" TEXT NOT NULL,
    "value" REAL NOT NULL,
    "lasted_ms" INTEGER CHECK ("lasted_ms" >= 0),
    PRIMARY KEY ("metric_alerts_pos", "history_pos"),
    FOREIGN KEY ("metric_alerts_pos") REFERENCES "metric_alerts" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE TABLE "metric_dashboards" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "starter" INTEGER NOT NULL CHECK ("starter" IN (0, 1)),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "metric_dashboards_scopes" (
    "metric_dashboards_pos" INTEGER NOT NULL CHECK ("metric_dashboards_pos" >= 0),
    "scopes_pos" INTEGER NOT NULL CHECK ("scopes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("metric_dashboards_pos", "scopes_pos"),
    FOREIGN KEY ("metric_dashboards_pos") REFERENCES "metric_dashboards" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "metric_dashboards_pins" (
    "metric_dashboards_pos" INTEGER NOT NULL CHECK ("metric_dashboards_pos" >= 0),
    "pins_pos" INTEGER NOT NULL CHECK ("pins_pos" >= 0),
    "tab" TEXT NOT NULL,
    "scope" TEXT NOT NULL,
    "panel" TEXT NOT NULL,
    PRIMARY KEY ("metric_dashboards_pos", "pins_pos"),
    FOREIGN KEY ("metric_dashboards_pos") REFERENCES "metric_dashboards" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "metric_dashboards_recent" (
    "metric_dashboards_pos" INTEGER NOT NULL CHECK ("metric_dashboards_pos" >= 0),
    "recent_pos" INTEGER NOT NULL CHECK ("recent_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("metric_dashboards_pos", "recent_pos"),
    FOREIGN KEY ("metric_dashboards_pos") REFERENCES "metric_dashboards" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
