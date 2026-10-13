-- Releases the advisory lock taken by migration_lock.sql.
-- $1 = lock key (bigint)
SELECT pg_advisory_unlock($1)
