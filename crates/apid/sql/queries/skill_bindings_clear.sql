-- Removes every binding of skill ?1, before its bindings are written whole again.
-- ?1 = skill id
DELETE FROM "skill_bindings" WHERE "skill_id" = ?1
