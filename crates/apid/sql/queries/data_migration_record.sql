-- Records one domain's outcome: inserts its row, or replaces the outcome of an earlier run. The
-- started_at of the first run is kept.
-- $1 = domain (text), $2 = status (text: pending or verified), $3 = source (text: blob, file or none),
-- $4 = source_sha256 (text, or null), $5 = rows_expected (bigint), $6 = rows_loaded (bigint), $7 = detail (text)
INSERT INTO data_migrations (domain, status, source, source_sha256, rows_expected, rows_loaded, detail, finished_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, CASE WHEN $2 = 'verified' THEN now() END)
ON CONFLICT (domain) DO UPDATE SET
    status = EXCLUDED.status,
    source = EXCLUDED.source,
    source_sha256 = EXCLUDED.source_sha256,
    rows_expected = EXCLUDED.rows_expected,
    rows_loaded = EXCLUDED.rows_loaded,
    detail = EXCLUDED.detail,
    finished_at = EXCLUDED.finished_at
