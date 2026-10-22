-- How the runs of each seat, model and engine did: the runs, the successes, and the cost and wall time of all of them.
SELECT "seat", "model", "engine", COUNT(*), SUM("ok"), SUM("cost_micros"), SUM("wall_ms")
FROM "eval_run"
GROUP BY "seat", "model", "engine"
ORDER BY "seat", "model", "engine"
