-- Test only: whether the data_migrations table exists. No parameters.
SELECT to_regclass('data_migrations') IS NOT NULL
