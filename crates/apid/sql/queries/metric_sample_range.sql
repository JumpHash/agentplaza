-- The 10-second points of one metric at times in [?2, ?3), in time and series order: ?1 metric.
SELECT series, t_ms, value FROM metric_sample
WHERE metric = ?1 AND t_ms >= ?2 AND t_ms < ?3
ORDER BY t_ms, series;
