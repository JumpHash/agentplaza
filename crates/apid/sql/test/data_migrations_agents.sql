-- Test only: the agents row as "status source". No parameters.
SELECT status || ' ' || source FROM data_migrations WHERE domain = 'agents'
