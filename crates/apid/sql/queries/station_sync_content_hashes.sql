-- The content hash of every sync row, each once: the hub's copy of each path and the version both sides last agreed.
-- Takes no parameters.
SELECT "hash" FROM "station_sync_file" WHERE "hash" IS NOT NULL
UNION
SELECT "base" FROM "station_sync_file" WHERE "base" IS NOT NULL
