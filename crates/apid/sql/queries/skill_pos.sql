-- The place of skill ?1 in the skill table: its own when it has one, else the next one after the last. Keeps a skill's
-- place when it is written again.
-- ?1 = skill id
SELECT COALESCE((SELECT "pos" FROM "skill" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "skill"))
