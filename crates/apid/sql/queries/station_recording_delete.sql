-- Deletes one recording's index row. Its actions and frames are deleted by their own statements. ?1 = id.
DELETE FROM "station_recording" WHERE "id" = ?1
