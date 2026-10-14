-- Appends one action to a recording's events (the events' seq is generated).
-- $1 = recording (text), $2 = at_ms (bigint), $3 = kind (text), $4 = ok (boolean), $5 = summary (jsonb)
INSERT INTO station_recording_events (recording, at_ms, kind, ok, summary) VALUES ($1,$2,$3,$4,$5)
