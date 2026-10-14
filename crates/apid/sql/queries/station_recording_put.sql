-- Writes one recording's index row: inserted when the recording starts, then updated with its end and counts.
-- $1 = id (text), $2 = station (text), $3 = lease (text), $4 = agent (bigint), $5 = task (text, null when none),
-- $6 = started_ms (bigint), $7 = ended_ms (bigint, null while open), $8 = frames (bigint), $9 = actions (bigint),
-- $10 = bytes (bigint), $11 = takeover (boolean), $12 = width (bigint, null when no frame), $13 = height (bigint)
INSERT INTO station_recordings (id, station, lease, agent, task, started_ms, ended_ms, frames, actions, bytes, takeover, width, height)
VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
ON CONFLICT (id) DO UPDATE SET ended_ms = EXCLUDED.ended_ms, frames = EXCLUDED.frames, actions = EXCLUDED.actions,
    bytes = EXCLUDED.bytes, takeover = EXCLUDED.takeover, width = EXCLUDED.width, height = EXCLUDED.height
