-- The task passed qualification stages in the lab: ?1 the task, ?2 the stages, ?3 the lab minutes they took.
UPDATE "eval_run" SET "qual_stages" = "qual_stages" + ?2, "lab_minutes" = "lab_minutes" + ?3 WHERE "task" = ?1
