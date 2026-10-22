-- The 1-hour windows of one metric with starts in [?2, ?3), in time and series order: ?1 metric.
SELECT series, window_ms, count, sum, min, max, last, hist_count, hist_sum, hist_min, hist_max FROM metric_agg_1h
WHERE metric = ?1 AND window_ms >= ?2 AND window_ms < ?3
ORDER BY window_ms, series;
