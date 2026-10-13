-- Appends one transcript record; a duplicate (cid, seq) is dropped, not an error.
-- $1 = cid (bigint), $2 = seq (bigint), $3 = work (bigint), $4 = kind (text),
-- $5 = at_ms (bigint), $6 = data (jsonb)
INSERT INTO transcript (cid, seq, work, kind, at_ms, data) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING
