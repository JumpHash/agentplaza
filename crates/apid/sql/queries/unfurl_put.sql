-- Stores one answer, replacing the cached one (unfurl/mod.rs). $1 = key (text), $2 = data (jsonb), $3 = ok (bool),
-- $4 = fetched_ms (bigint)
INSERT INTO unfurl_cache (key, data, ok, fetched_ms) VALUES ($1,$2,$3,$4)
ON CONFLICT (key) DO UPDATE SET data = EXCLUDED.data, ok = EXCLUDED.ok, fetched_ms = EXCLUDED.fetched_ms
