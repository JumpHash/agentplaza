-- The newest actions of one recording, oldest first. ?1 = recording, ?2 = the most rows.
SELECT * FROM (
    SELECT * FROM "station_recording_event" WHERE "recording" = ?1 ORDER BY "seq" DESC LIMIT ?2
) newest ORDER BY "seq"
