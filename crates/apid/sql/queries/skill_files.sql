-- The files of version ?2 of skill ?1, by path.
-- ?1 = skill id, ?2 = version
SELECT * FROM "skill_file" WHERE "skill_id" = ?1 AND "version" = ?2 ORDER BY "path"
