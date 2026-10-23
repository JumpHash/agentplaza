-- The live prompt holding binding ?1, if any (a binding is unique among live prompts). Its tags come from prompt_tags.sql.
-- ?1 = binding
SELECT * FROM "prompt" WHERE "binding" = ?1 AND "archived_ms" IS NULL LIMIT 1
