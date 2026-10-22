-- Deletes the kept traces that start before ?1 (ms); their spans and attributes go with them.
DELETE FROM trace WHERE start_ms < ?1;
