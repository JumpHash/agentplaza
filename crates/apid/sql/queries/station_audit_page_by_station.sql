-- The newest station audit rows of one workstation, newest first (served by station_audit_station_at_ms). ?1 =
-- station, ?2 = the most rows.
SELECT * FROM "station_audit" WHERE "station" = ?1 ORDER BY "at_ms" DESC, "seq" DESC LIMIT ?2
