-- One work item's totals: ?1 cid, ?2 work.
SELECT "totals_calls", "totals_tokens_input", "totals_tokens_output", "totals_tokens_cache_read",
    "totals_tokens_cache_write", "totals_cost_usd", "totals_est_usd", "totals_usd"
FROM "usage_work" WHERE "cid" = ?1 AND "work" = ?2
