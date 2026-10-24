-- How many failed unlocks share ?1 has had after ?2 (Unix ms): the window is the UNLOCK_WINDOW_MS before an attempt, and
-- a failure exactly at its start is outside it.
-- ?1 = share id, ?2 = the window's start, in Unix ms
SELECT COUNT(*) FROM "share_event" WHERE "share" = ?1 AND "kind" = 'unlock_failed' AND "at_ms" > ?2
