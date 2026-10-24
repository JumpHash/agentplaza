-- Every eval record the filter selects, oldest first (what the cost and reviewer reports fold over): ?1 project, ?2
-- model and ?3 seat (NULL for any).
SELECT "id", "session", "project", "task", "recipe", "step", "seat", "round", "engine", "engine_version", "model",
    "provider", "harness", "node", "pool", "tokens_in", "tokens_out", "tokens_cached", "cost_micros", "model_calls",
    "tool_calls", "tool_ms", "wall_ms", "verdict", "outcome", "ok", "landed", "reverted", "ended_ms", "recipe_version",
    "harness_version", "gate", "later_findings", "incidents", "memory_cites", "qual_stages", "lab_minutes"
FROM "eval_run"
WHERE ("project" = ?1 OR ?1 IS NULL) AND ("model" = ?2 OR ?2 IS NULL) AND ("seat" = ?3 OR ?3 IS NULL)
ORDER BY "id"
