-- Every prompt, by id: the live ones, and the archived ones too when ?1 is 1. Its tags come from prompt_tags.sql.
-- ?1 = include archived (0 or 1)
SELECT * FROM "prompt" WHERE ?1 = 1 OR "archived_ms" IS NULL ORDER BY "id"
