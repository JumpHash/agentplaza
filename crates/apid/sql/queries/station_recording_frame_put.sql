-- Writes one frame's row: its capture time and size. Idempotent per (recording, seq).
-- $1 = recording (text), $2 = seq (bigint), $3 = ts_ms (bigint), $4 = bytes (bigint)
INSERT INTO station_recording_frames (recording, seq, ts_ms, bytes) VALUES ($1,$2,$3,$4)
ON CONFLICT (recording, seq) DO UPDATE SET ts_ms = EXCLUDED.ts_ms, bytes = EXCLUDED.bytes
