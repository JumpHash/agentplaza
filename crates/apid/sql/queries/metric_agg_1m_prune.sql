-- Deletes the 1-minute windows that start before ?1 (ms); their buckets go with them.
DELETE FROM metric_agg_1m WHERE window_ms < ?1;
