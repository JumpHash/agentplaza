-- The histogram buckets of the 1-hour windows of one metric with starts in [?2, ?3), in order: ?1 metric.
SELECT series, window_ms, bucket, count FROM metric_agg_1h_bucket
WHERE metric = ?1 AND window_ms >= ?2 AND window_ms < ?3
ORDER BY window_ms, series, bucket;
