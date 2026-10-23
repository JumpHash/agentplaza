-- The body of version ?2 of prompt ?1 (no row when there is no such version).
-- ?1 = prompt id, ?2 = version
SELECT "body" FROM "prompt_version" WHERE "prompt_id" = ?1 AND "version" = ?2
