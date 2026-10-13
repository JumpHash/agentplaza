-- The newest records of one agent's transcript, newest first (the caller reverses them).
-- $1 = cid (bigint), $2 = limit (bigint)
SELECT seq, work, kind, at_ms, data FROM transcript WHERE cid = $1 ORDER BY seq DESC LIMIT $2
