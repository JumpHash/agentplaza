-- The recordings, newest first (by start, then id): one workstation's when ?1 is given (NULL for any), and only those
-- started before ?2 (ms) when it is given (NULL for none). ?3 = the most rows.
SELECT * FROM "station_recording"
WHERE ("station" = ?1 OR ?1 IS NULL) AND ("started_ms" < ?2 OR ?2 IS NULL)
ORDER BY "started_ms" DESC, "id" DESC
LIMIT ?3
