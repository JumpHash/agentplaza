-- The tags of skill ?1, in the order they were given.
-- ?1 = skill id
SELECT "value" FROM "skill_tags" WHERE "skill_id" = ?1 ORDER BY "tags_pos"
