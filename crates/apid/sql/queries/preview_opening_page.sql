-- The most recent openings of link ?1, newest first, at most ?2 of them.
-- ?1 = link id, ?2 = limit
SELECT * FROM "preview_opening" WHERE "link" = ?1 ORDER BY "seq" DESC LIMIT ?2
