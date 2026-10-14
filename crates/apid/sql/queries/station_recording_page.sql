-- The recordings, newest first (by start, then id): one workstation's when $1 is given, and only those started before
-- $2 (ms) when it is given. $1 = station (text, null for any), $2 = before (bigint, null for none), $3 = limit (bigint)
SELECT id, station, lease, agent, task, started_ms, ended_ms, frames, actions, bytes, takeover, width, height
FROM station_recordings
WHERE ($1::text IS NULL OR station = $1) AND ($2::bigint IS NULL OR started_ms < $2)
ORDER BY started_ms DESC, id DESC
LIMIT $3
