-- The review passes: each reviewer session that passed its task (the model, the task, when the session ended).
SELECT "model", "task", "ended_ms"
FROM "eval_run"
WHERE "seat" = 'reviewer' AND "verdict" = 1 AND "model" IS NOT NULL AND "task" IS NOT NULL
