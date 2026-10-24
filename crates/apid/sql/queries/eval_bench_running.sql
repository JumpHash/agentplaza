-- The running replay of a task: ?1 the task. Its id, the case's known outcome, when it started, its test command and the
-- result of that command.
SELECT "id", "known", "replayed_ms", "command", "check" FROM "eval_bench" WHERE "task" = ?1 AND "outcome" = 'running'
