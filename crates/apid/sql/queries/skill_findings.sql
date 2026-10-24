-- The check findings of version ?2 of skill ?1, in the order the check made them.
-- ?1 = skill id, ?2 = version
SELECT * FROM "skill_finding" WHERE "skill_id" = ?1 AND "version" = ?2 ORDER BY "seq"
