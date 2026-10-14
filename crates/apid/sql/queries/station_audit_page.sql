-- The newest station audit rows of the whole plaza, newest first (ties at the same at_ms go to the later row).
-- $1 = limit (bigint)
SELECT data FROM station_audit ORDER BY at_ms DESC, seq DESC LIMIT $1
