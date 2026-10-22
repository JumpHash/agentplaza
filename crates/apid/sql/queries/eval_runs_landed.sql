-- Sets whether a task's change landed on every run of the task: ?1 the task, ?2 whether it landed.
UPDATE "eval_run" SET "landed" = ?2 WHERE "task" = ?1
