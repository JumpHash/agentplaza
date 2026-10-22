-- Every sync row of one workstation, by path (deleted rows included: they carry the deletion to the station). ?1 =
-- station.
SELECT * FROM "station_sync_file" WHERE "station" = ?1 ORDER BY "path"
