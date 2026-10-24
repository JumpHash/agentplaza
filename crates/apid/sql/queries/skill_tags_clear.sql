-- Removes every tag of skill ?1, before its tags are written whole again.
-- ?1 = skill id
DELETE FROM "skill_tags" WHERE "skill_id" = ?1
