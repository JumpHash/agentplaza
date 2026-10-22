-- Deletes every frame row of one recording. ?1 = recording.
DELETE FROM "station_recording_frame" WHERE "recording" = ?1
