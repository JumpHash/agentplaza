-- Test only: the agents data migration row as "status source rows_expected rows_loaded sha_length".
SELECT status || ' ' || source || ' ' || rows_expected || ' ' || rows_loaded || ' ' || coalesce(length(source_sha256), 0) FROM data_migrations WHERE domain = 'agents'
