-- Deletes the 1-hour windows that start before ?1 (ms); their buckets go with them.
DELETE FROM metric_agg_1h WHERE window_ms < ?1;
