-- Puts one 10-second point of a series (a point at the same time is replaced): ?1 metric, ?2 t_ms, ?3 series id, ?4 value.
INSERT INTO metric_sample (metric, t_ms, series, value) VALUES (?1, ?2, ?3, ?4)
ON CONFLICT (metric, t_ms, series) DO UPDATE SET value = excluded.value;
