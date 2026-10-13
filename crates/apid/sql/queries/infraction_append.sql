-- Appends one infraction (a law refused an agent's act).
-- $1 = cid (bigint), $2 = law (text), $3 = at_ms (bigint), $4 = tool (text), $5 = detail (text)
INSERT INTO infraction (cid, law, at_ms, tool, detail) VALUES ($1,$2,$3,$4,$5)
