-- Whether a lab run was already recorded: ?1 the run's number.
SELECT COUNT(*) FROM "eval_qualification" WHERE "id" = ?1
