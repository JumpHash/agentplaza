-- Deletes the histogram buckets of one 1-hour window before its buckets are written again: ?1 metric, ?2 window start ms, ?3 series id.
DELETE FROM metric_agg_1h_bucket WHERE metric = ?1 AND window_ms = ?2 AND series = ?3;
