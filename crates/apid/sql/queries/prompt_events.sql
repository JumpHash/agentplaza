-- The history of prompt ?1, newest first, at most ?2 rows.
-- ?1 = prompt id, ?2 = limit
SELECT * FROM "prompt_event" WHERE "prompt_id" = ?1 ORDER BY "id" DESC LIMIT ?2
