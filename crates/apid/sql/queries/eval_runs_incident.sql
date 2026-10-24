-- An incident traced back to the task's landing: ?1 the task. Every run of the task counts it.
UPDATE "eval_run" SET "incidents" = "incidents" + 1 WHERE "task" = ?1
