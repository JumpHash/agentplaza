-- The next id of an archived case.
SELECT COALESCE(MAX("id"), 0) + 1 FROM "eval_archive"
