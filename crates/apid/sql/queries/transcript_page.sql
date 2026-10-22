-- The newest stored entries of one agent, newest first (transcript.rs reverses them). Every column of `entry`, in the
-- table's order (its key, then its columns: transcript.rs reads each row by position). ?1 = cid, ?2 = the most entries.
SELECT * FROM "entry" WHERE "cid" = ?1 ORDER BY "seq" DESC LIMIT ?2
