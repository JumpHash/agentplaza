-- Adds one call's totals to its hour; the hour's row is made when it is new. The row's pos is its list order, unused
-- for a stream: 0. ?1 hour_ms, ?2 model, ?3 cid, ?4 provider, ?5 role, ?6 calls, ?7 input, ?8 output, ?9 cache_read,
-- ?10 cache_write, ?11 cost_usd, ?12 est_usd, ?13 usd.
INSERT INTO "usage_hour" ("hour_ms", "model", "cid", "provider", "role", "totals_calls", "totals_tokens_input",
    "totals_tokens_output", "totals_tokens_cache_read", "totals_tokens_cache_write", "totals_cost_usd",
    "totals_est_usd", "totals_usd", "pos")
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)
ON CONFLICT ("hour_ms", "model", "cid") DO UPDATE SET
    "totals_calls" = "usage_hour"."totals_calls" + excluded."totals_calls",
    "totals_tokens_input" = "usage_hour"."totals_tokens_input" + excluded."totals_tokens_input",
    "totals_tokens_output" = "usage_hour"."totals_tokens_output" + excluded."totals_tokens_output",
    "totals_tokens_cache_read" = "usage_hour"."totals_tokens_cache_read" + excluded."totals_tokens_cache_read",
    "totals_tokens_cache_write" = "usage_hour"."totals_tokens_cache_write" + excluded."totals_tokens_cache_write",
    "totals_cost_usd" = "usage_hour"."totals_cost_usd" + excluded."totals_cost_usd",
    "totals_est_usd" = "usage_hour"."totals_est_usd" + excluded."totals_est_usd",
    "totals_usd" = "usage_hour"."totals_usd" + excluded."totals_usd"
