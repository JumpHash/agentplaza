-- The shares of one target, newest first.
-- ?1 = target kind ("artifact" or "file"), ?2 = target id
SELECT * FROM "share" WHERE "target_kind" = ?1 AND "target_id" = ?2 ORDER BY "pos" DESC
