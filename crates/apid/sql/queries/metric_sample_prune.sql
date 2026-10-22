-- Deletes the 10-second points before ?1 (ms).
DELETE FROM metric_sample WHERE t_ms < ?1;
