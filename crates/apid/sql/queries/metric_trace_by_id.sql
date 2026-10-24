-- The newest kept trace with this id: ?1 trace id.
SELECT seq, id, cid, role, start_ms, dur_ms, ok, kept FROM trace WHERE id = ?1 ORDER BY seq DESC LIMIT 1;
