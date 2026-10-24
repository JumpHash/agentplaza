-- A `run` a replay's VM reported: ?1 the task, ?2 the command, ?3 whether it passed. The running replay that names the
-- command keeps the result of its last run.
UPDATE "eval_bench" SET "check" = ?3 WHERE "task" = ?1 AND "outcome" = 'running' AND "command" = ?2 AND "command" <> ''
