-- The newest infractions of one agent, newest first.
-- $1 = cid (bigint), $2 = limit (bigint)
SELECT cid, law, at_ms, tool, detail FROM infraction WHERE cid = $1 ORDER BY at_ms DESC, id DESC LIMIT $2
