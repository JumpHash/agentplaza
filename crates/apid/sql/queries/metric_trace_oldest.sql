-- The kept traces that start before ?1 (ms), oldest first: sequence and bytes.
SELECT seq, bytes FROM trace WHERE start_ms < ?1 ORDER BY start_ms, seq;
