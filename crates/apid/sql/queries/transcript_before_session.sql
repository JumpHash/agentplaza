-- The newest stored entries of one session on one agent before a seq, newest first (transcript.rs reverses them). Every
-- column of `entry`, in the table's order (transcript.rs reads each row by position). ?1 = cid, ?2 = the seq they come
-- before, ?3 = the session, ?4 = the most entries.
SELECT * FROM "entry" WHERE "cid" = ?1 AND "seq" < ?2 AND "session" = ?3 ORDER BY "seq" DESC LIMIT ?4
