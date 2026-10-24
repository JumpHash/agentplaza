-- Version ?2 of skill ?1 with its SKILL.md, if there is such a version. Its files come from skill_files.sql and its
-- findings from skill_findings.sql.
-- ?1 = skill id, ?2 = version
SELECT * FROM "skill_version" WHERE "skill_id" = ?1 AND "version" = ?2
