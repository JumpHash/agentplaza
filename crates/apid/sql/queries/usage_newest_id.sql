-- The newest call's id (0 when there is none): the counter must never hand out an id a stored call holds.
SELECT COALESCE(MAX("id"), 0) FROM "usage_call"
