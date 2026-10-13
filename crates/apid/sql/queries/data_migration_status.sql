-- The recorded status of one data migration domain, if it has a row.
-- $1 = domain (text)
SELECT status FROM data_migrations WHERE domain = $1
