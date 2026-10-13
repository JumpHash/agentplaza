-- Takes the session-level advisory lock that serialises migration runners. Blocks until free.
-- $1 = lock key (bigint)
SELECT pg_advisory_lock($1)
