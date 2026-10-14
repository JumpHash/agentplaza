-- The newest owner actions of the whole plaza, newest first.
-- $1 = limit (bigint)
SELECT data FROM ide_audit ORDER BY at_ms DESC, seq DESC LIMIT $1
