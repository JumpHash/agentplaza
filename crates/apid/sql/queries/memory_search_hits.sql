-- ?1 the match expression (words, each quoted), ?2 the project, ?3 the type ('' for any), ?4 the anchor ('' for any).
-- The project's live entries that match, best match first (store/search.rs).
SELECT "memory_entry_search"."key"
FROM "memory_entry_search"
JOIN "memory_entry" AS "e" ON "e"."key" = "memory_entry_search"."key"
WHERE "memory_entry_search" MATCH ?1
  AND "e"."project" = ?2
  AND "e"."status" IN ('active', 'flagged')
  AND (?3 = '' OR "e"."kind" = ?3)
  AND (?4 = '' OR EXISTS (
    SELECT 1 FROM "memory_entry_anchors" AS "a"
    WHERE "a"."memory_entry_key" = "e"."key" AND "a"."value" = ?4))
ORDER BY bm25("memory_entry_search"), "e"."key"
