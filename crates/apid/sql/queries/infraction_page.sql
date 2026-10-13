-- The newest infractions of the whole plaza, newest first.
-- $1 = limit (bigint)
SELECT cid, law, at_ms, tool, detail FROM infraction ORDER BY at_ms DESC, id DESC LIMIT $1
