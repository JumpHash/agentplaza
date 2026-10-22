-- One frame's row: its capture time, size and the hash of its JPEG. ?1 = recording, ?2 = seq.
SELECT * FROM "station_recording_frame" WHERE "recording" = ?1 AND "seq" = ?2
