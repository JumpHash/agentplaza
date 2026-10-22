-- The prompt with id ?1, without its tags (prompt_tags.sql reads them).
-- ?1 = prompt id
SELECT * FROM "prompt" WHERE "id" = ?1
