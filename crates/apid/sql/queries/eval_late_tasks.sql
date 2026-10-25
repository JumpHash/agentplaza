-- The tasks that later drew an incident or were reverted.
SELECT DISTINCT "task" FROM "eval_run" WHERE "task" IS NOT NULL AND ("incidents" > 0 OR "reverted" = 1)
