-- Writes one sync row, inserted or replaced by (station, path). A pass writes its changed rows in one transaction.
-- $1 = station (text), $2 = path (text), $3 = hash (text, null when deleted), $4 = size (bigint), $5 = mtime_ms (bigint),
-- $6 = version (bigint), $7 = base (text, null when absent on both sides), $8 = conflict_of (text, null unless a conflict copy),
-- $9 = synced_ms (bigint)
INSERT INTO station_sync_files (station, path, hash, size, mtime_ms, version, base, conflict_of, synced_ms)
VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
ON CONFLICT (station, path) DO UPDATE SET hash = EXCLUDED.hash, size = EXCLUDED.size, mtime_ms = EXCLUDED.mtime_ms,
    version = EXCLUDED.version, base = EXCLUDED.base, conflict_of = EXCLUDED.conflict_of, synced_ms = EXCLUDED.synced_ms
