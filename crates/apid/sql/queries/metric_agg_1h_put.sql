-- Puts one closed 1-hour window (a window of the same start is replaced): ?1 metric, ?2 window start ms, ?3 series id, ?4 count, ?5 sum, ?6 min, ?7 max, ?8 last, ?9 histogram count (NULL without one), ?10 histogram sum, ?11 histogram min, ?12 histogram max.
INSERT INTO metric_agg_1h (metric, window_ms, series, count, sum, min, max, last, hist_count, hist_sum, hist_min, hist_max)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT (metric, window_ms, series) DO UPDATE SET
    count = excluded.count, sum = excluded.sum, min = excluded.min, max = excluded.max, last = excluded.last,
    hist_count = excluded.hist_count, hist_sum = excluded.hist_sum, hist_min = excluded.hist_min, hist_max = excluded.hist_max;
