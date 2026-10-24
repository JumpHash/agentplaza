-- A finding was confirmed on the task: ?1 the task. The builder's runs carry it (the reviewers' runs do not).
UPDATE "eval_run" SET "later_findings" = "later_findings" + 1 WHERE "task" = ?1 AND "seat" NOT IN ('reviewer', 'verifier')
