-- One page of kept traces starting at times in [?1, ?2), newest first: ?3 the most rows, ?4 the rows to skip.
SELECT seq, id, cid, role, start_ms, dur_ms, ok, kept FROM trace
WHERE start_ms >= ?1 AND start_ms < ?2
ORDER BY start_ms DESC, seq DESC
LIMIT ?3 OFFSET ?4;
