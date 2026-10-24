-- Every archived case, oldest first.
SELECT "id", "project", "task", "start", "goal", "check", "command", "outcome", "archived_ms"
FROM "eval_archive"
ORDER BY "id"
