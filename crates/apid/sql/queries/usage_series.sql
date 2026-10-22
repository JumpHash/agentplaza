-- The usage of the hours in [?2, ?4) by bucket (idx, counted from ?2 in ?3-ms buckets) and group (grp, as usage_summary).
-- ?1 is the grouping.
SELECT ("hour_ms" - ?2) / ?3 AS "idx",
    (CASE ?1 WHEN 'agent' THEN CAST("cid" AS TEXT) WHEN 'role' THEN "role" WHEN 'provider' THEN "provider"
        ELSE "model" END) AS "grp",
    COALESCE(SUM("totals_calls"), 0) AS "calls",
    COALESCE(SUM("totals_tokens_input"), 0) AS "input",
    COALESCE(SUM("totals_tokens_output"), 0) AS "output",
    COALESCE(SUM("totals_tokens_cache_read"), 0) AS "cache_read",
    COALESCE(SUM("totals_tokens_cache_write"), 0) AS "cache_write",
    COALESCE(SUM("totals_cost_usd"), 0.0) AS "cost_usd",
    COALESCE(SUM("totals_est_usd"), 0.0) AS "est_usd",
    COALESCE(SUM("totals_usd"), 0.0) AS "usd"
FROM "usage_hour"
WHERE "hour_ms" >= ?2 AND "hour_ms" < ?4
GROUP BY "idx", "grp"
