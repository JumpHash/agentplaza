-- What the sessions of a task spent: ?1 the task (micro-dollars).
SELECT COALESCE(SUM("cost_micros"), 0) FROM "eval_run" WHERE "task" = ?1
