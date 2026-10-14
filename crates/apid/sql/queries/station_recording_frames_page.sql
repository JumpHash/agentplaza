-- The capture times of one recording's frames, in seq order: the frames its index counts (seq up to $2), at most $3.
-- $1 = recording (text), $2 = upto (bigint), $3 = limit (bigint)
SELECT seq, ts_ms FROM station_recording_frames WHERE recording = $1 AND seq <= $2 ORDER BY seq LIMIT $3
