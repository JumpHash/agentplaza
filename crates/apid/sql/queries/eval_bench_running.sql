-- The running replay of a task: ?1 the task. Its id, the case's known outcome and when it started.
SELECT "id", "known", "replayed_ms" FROM "eval_bench" WHERE "task" = ?1 AND "outcome" = 'running'
