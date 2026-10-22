-- The newest seq of one recording's actions (0 when it has none); the next action takes this + 1. ?1 = recording.
SELECT COALESCE(MAX("seq"), 0) FROM "station_recording_event" WHERE "recording" = ?1
