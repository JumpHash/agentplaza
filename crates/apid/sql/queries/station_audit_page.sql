-- The newest station audit rows of the whole plaza, newest first (a tie at the same at_ms goes to the later row).
-- ?1 = the most rows.
SELECT * FROM "station_audit" ORDER BY "at_ms" DESC, "seq" DESC LIMIT ?1
