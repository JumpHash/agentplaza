-- The most recent events of share ?1, newest first, at most ?2 of them.
-- ?1 = share id, ?2 = limit
SELECT * FROM "share_event" WHERE "share" = ?1 ORDER BY "seq" DESC LIMIT ?2
