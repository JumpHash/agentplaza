-- One recording's index row. $1 = id (text)
SELECT id, station, lease, agent, task, started_ms, ended_ms, frames, actions, bytes, takeover, width, height
FROM station_recordings WHERE id = $1
