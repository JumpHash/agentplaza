-- Every finding ruling, oldest first.
SELECT "id", "project", "task", "review", "finding", "reviewer", "reviewer_seat", "verifier", "verdict", "severity", "ruled_ms"
FROM "eval_finding"
ORDER BY "id"
