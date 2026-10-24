-- How the runs of each step, seat, model and engine did: the runs, the successes, the cost and wall time of all of them,
-- and the clean successes (no revert, no later finding, no incident: see report::clean).
SELECT "step", "seat", "model", "engine", COUNT(*), SUM("ok"), SUM("cost_micros"), SUM("wall_ms"),
    SUM(CASE WHEN "ok" = 1 AND "reverted" IS NOT 1 AND "later_findings" = 0 AND "incidents" = 0 THEN 1 ELSE 0 END)
FROM "eval_run"
GROUP BY "step", "seat", "model", "engine"
ORDER BY "step", "seat", "model", "engine"
