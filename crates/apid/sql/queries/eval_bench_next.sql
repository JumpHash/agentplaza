-- The next id of a replay.
SELECT COALESCE(MAX("id"), 0) + 1 FROM "eval_bench"
