-- Every skill, by id: the live ones, and the archived ones too when ?1 is 1. Its lists come from skill_tags.sql and
-- skill_bindings.sql.
-- ?1 = include archived (0 or 1)
SELECT * FROM "skill" WHERE ?1 = 1 OR "archived_ms" IS NULL ORDER BY "id"
