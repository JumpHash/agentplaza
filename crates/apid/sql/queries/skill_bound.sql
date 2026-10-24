-- The live, published skills holding binding ?1, by name (their lists come from skill_tags.sql and
-- skill_bindings.sql; the live version's metadata from skill_version_meta.sql).
-- ?1 = binding (role:<role> or plaza)
SELECT * FROM "skill"
WHERE "archived_ms" IS NULL AND "active_version" IS NOT NULL
  AND "id" IN (SELECT "skill_id" FROM "skill_bindings" WHERE "value" = ?1)
ORDER BY "name"
