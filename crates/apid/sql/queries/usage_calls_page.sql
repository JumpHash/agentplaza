-- A page of calls, newest first: ?1 model (NULL for any), ?2 cid (NULL for any), ?3 the id below which to start (NULL
-- for the newest), ?4 how many rows to return.
SELECT "id", "at_ms", "cid", "role", "model", "provider", "tokens_input", "tokens_output", "tokens_cache_read",
    "tokens_cache_write", "cost_usd", "est_usd", "task", "work", "via", "rung", "escalated"
FROM "usage_call"
WHERE ("model" = ?1 OR ?1 IS NULL) AND ("cid" = ?2 OR ?2 IS NULL) AND ("id" < ?3 OR ?3 IS NULL)
ORDER BY "id" DESC
LIMIT ?4
