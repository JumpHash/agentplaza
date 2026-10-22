-- The frame rows of one recording in seq order: the frames its index counts (seq up to ?2), at most ?3 rows.
-- ?1 = recording, ?2 = upto, ?3 = the most rows.
SELECT * FROM "station_recording_frame" WHERE "recording" = ?1 AND "seq" <= ?2 ORDER BY "seq" LIMIT ?3
