-- 0014 metrics: the metrics tables (crates/apid/src/metrics/pg.rs). Every table but metric_kv is partitioned by day
-- (day = the UTC date of the row's time); the store creates each day's partition on demand and retention drops
-- whole partitions. metric_chunk_10s holds one row per series per 10 minutes, the points chunk-encoded
-- (apz_metrics::chunk). metric_1m holds one row per series per 10 minutes and metric_1h one per series per day,
-- the windows as deflate-compressed JSON. metric_events, model_calls and trace_spans hold one row per record, the whole record
-- as jsonb, with the columns the readers filter and order by. metric_kv holds the small saved values.
CREATE TABLE metric_chunk_10s (
    day date NOT NULL,
    series text NOT NULL,
    start_ms bigint NOT NULL,
    points bytea NOT NULL,
    PRIMARY KEY (day, series, start_ms)
) PARTITION BY RANGE (day);

CREATE TABLE metric_1m (
    day date NOT NULL,
    series text NOT NULL,
    start_ms bigint NOT NULL,
    aggs bytea NOT NULL,
    PRIMARY KEY (day, series, start_ms)
) PARTITION BY RANGE (day);

CREATE TABLE metric_1h (
    day date NOT NULL,
    series text NOT NULL,
    day_ms bigint NOT NULL,
    aggs bytea NOT NULL,
    PRIMARY KEY (day, series, day_ms)
) PARTITION BY RANGE (day);

CREATE TABLE metric_events (
    day date NOT NULL,
    t_ms bigint NOT NULL,
    kind text NOT NULL,
    entity text NOT NULL,
    data jsonb NOT NULL
) PARTITION BY RANGE (day);
CREATE INDEX metric_events_t_ms ON metric_events (t_ms);

CREATE TABLE model_calls (
    day date NOT NULL,
    at_ms bigint NOT NULL,
    cid bigint NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    data jsonb NOT NULL
) PARTITION BY RANGE (day);
CREATE INDEX model_calls_at_ms ON model_calls (at_ms);

CREATE TABLE trace_spans (
    day date NOT NULL,
    start_ms bigint NOT NULL,
    id text NOT NULL,
    cid bigint NOT NULL,
    data jsonb NOT NULL,
    bytes int NOT NULL
) PARTITION BY RANGE (day);
CREATE INDEX trace_spans_id ON trace_spans (id);

CREATE TABLE metric_kv (
    key text PRIMARY KEY,
    value text NOT NULL,
    at_ms bigint NOT NULL
);
