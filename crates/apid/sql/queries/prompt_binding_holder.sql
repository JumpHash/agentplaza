-- The id of a live prompt other than ?2 that holds binding ?1 (no row when the binding is free).
-- ?1 = binding (role:<role>, plaza or project:<id>), ?2 = the prompt's own id
SELECT "id" FROM "prompt" WHERE "binding" = ?1 AND "id" <> ?2 AND "archived_ms" IS NULL LIMIT 1
