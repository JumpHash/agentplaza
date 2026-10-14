-- Every sync row of one workstation, by path (deleted rows included: they carry the deletion to the station).
-- $1 = station (text)
SELECT path, hash, size, mtime_ms, version, base, conflict_of, synced_ms
FROM station_sync_files
WHERE station = $1
ORDER BY path
