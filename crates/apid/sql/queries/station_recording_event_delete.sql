-- Deletes every action of one recording. ?1 = recording.
DELETE FROM "station_recording_event" WHERE "recording" = ?1
