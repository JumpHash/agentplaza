-- The next id of a finding ruling.
SELECT COALESCE(MAX("id"), 0) + 1 FROM "eval_finding"
