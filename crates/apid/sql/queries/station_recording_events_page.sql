-- The newest events of one recording, oldest first. $1 = recording (text), $2 = limit (bigint)
SELECT at_ms, kind, ok, summary FROM (
    SELECT seq, at_ms, kind, ok, summary FROM station_recording_events WHERE recording = $1 ORDER BY seq DESC LIMIT $2
) newest ORDER BY seq
