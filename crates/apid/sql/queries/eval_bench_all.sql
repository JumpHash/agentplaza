-- Every replay, oldest first (the route reverses it).
SELECT "id", "archive", "task", "reference", "command", "check", "outcome", "known", "matched", "cost_micros", "wall_ms", "replayed_ms"
FROM "eval_bench"
ORDER BY "id"
