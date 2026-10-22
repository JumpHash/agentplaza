-- The recordings of one workstation that ended before ?2 (ms), or never ended and started before it (a crash left them
-- open): the retention sweep's list, oldest first. ?1 = station, ?2 = cutoff (ms).
SELECT * FROM "station_recording"
WHERE "station" = ?1 AND COALESCE("ended_ms", "started_ms") < ?2
ORDER BY "started_ms" ASC, "id" ASC
