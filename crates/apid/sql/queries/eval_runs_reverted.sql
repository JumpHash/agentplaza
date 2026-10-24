-- The task's landed change was reverted: ?1 the task. Every run of the task says so.
UPDATE "eval_run" SET "reverted" = 1 WHERE "task" = ?1
