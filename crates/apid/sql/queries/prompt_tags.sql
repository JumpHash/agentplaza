-- The tags of prompt ?1, in the order they were given.
-- ?1 = prompt id
SELECT "value" FROM "prompt_tags" WHERE "prompt_id" = ?1 ORDER BY "tags_pos"
