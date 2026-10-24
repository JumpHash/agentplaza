-- Puts one histogram bucket of a 1-hour window: ?1 metric, ?2 window start ms, ?3 series id, ?4 bucket index, ?5 count.
INSERT INTO metric_agg_1h_bucket (metric, window_ms, series, bucket, count) VALUES (?1, ?2, ?3, ?4, ?5);
