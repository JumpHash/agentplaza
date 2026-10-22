-- Deletes the events before ?1 (ms); their parts go with them.
DELETE FROM metric_event WHERE t_ms < ?1;
