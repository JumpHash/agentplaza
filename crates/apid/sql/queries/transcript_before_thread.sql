-- The newest stored entries of one agent on one chat's thread before a seq, newest first (transcript.rs reverses them).
-- Every column of `entry`, in the table's order. The thread's entries are those of the work items whose start names it.
-- ?1 = cid, ?2 = the seq they come before, ?3 = the thread, ?4 = the most entries.
SELECT * FROM "entry" WHERE "cid" = ?1 AND "seq" < ?2 AND "work" IN (
    SELECT "work" FROM "entry" WHERE "cid" = ?1 AND "entry_kind" = 'work_start' AND "entry_work_start_thread" = ?3
) ORDER BY "seq" DESC LIMIT ?4
