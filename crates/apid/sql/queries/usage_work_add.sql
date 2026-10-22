-- Adds one call's totals to its work item (the row is made when it is new). pos is unused for a stream: 0.
-- ?1 cid, ?2 work, ?3 calls, ?4 input, ?5 output, ?6 cache_read, ?7 cache_write, ?8 cost_usd, ?9 est_usd, ?10 usd.
INSERT INTO "usage_work" ("cid", "work", "totals_calls", "totals_tokens_input", "totals_tokens_output",
    "totals_tokens_cache_read", "totals_tokens_cache_write", "totals_cost_usd", "totals_est_usd", "totals_usd", "pos")
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
ON CONFLICT ("cid", "work") DO UPDATE SET
    "totals_calls" = "usage_work"."totals_calls" + excluded."totals_calls",
    "totals_tokens_input" = "usage_work"."totals_tokens_input" + excluded."totals_tokens_input",
    "totals_tokens_output" = "usage_work"."totals_tokens_output" + excluded."totals_tokens_output",
    "totals_tokens_cache_read" = "usage_work"."totals_tokens_cache_read" + excluded."totals_tokens_cache_read",
    "totals_tokens_cache_write" = "usage_work"."totals_tokens_cache_write" + excluded."totals_tokens_cache_write",
    "totals_cost_usd" = "usage_work"."totals_cost_usd" + excluded."totals_cost_usd",
    "totals_est_usd" = "usage_work"."totals_est_usd" + excluded."totals_est_usd",
    "totals_usd" = "usage_work"."totals_usd" + excluded."totals_usd"
