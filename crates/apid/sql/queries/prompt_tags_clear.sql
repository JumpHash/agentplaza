-- Deletes the tags of prompt ?1; the same transaction writes them again.
-- ?1 = prompt id
DELETE FROM "prompt_tags" WHERE "prompt_id" = ?1
