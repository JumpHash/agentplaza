-- The Weft branch of the running replay of a task: ?1 the task.
SELECT "reference" FROM "eval_bench" WHERE "task" = ?1 AND "outcome" = 'running'
