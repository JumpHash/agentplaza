-- The newest station audit rows of one workstation, newest first (served by station_audit_station_at).
-- $1 = station (text), $2 = limit (bigint)
SELECT data FROM station_audit WHERE station = $1 ORDER BY at_ms DESC, seq DESC LIMIT $2
