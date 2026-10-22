-- The history of skill ?1, newest first, at most ?2 rows.
-- ?1 = skill id, ?2 = limit
SELECT * FROM "skill_event" WHERE "skill_id" = ?1 ORDER BY "id" DESC LIMIT ?2
