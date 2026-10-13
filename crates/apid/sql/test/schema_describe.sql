-- Test only: one text row per column, index and constraint of the public schema (except
-- schema_migrations), so two databases can be compared for identical structure. No parameters.
SELECT string_agg(line, E'\n' ORDER BY line) FROM (
    SELECT 'column ' || table_name || '.' || column_name || ' ' || data_type
        || ' nullable=' || is_nullable || ' default=' || coalesce(column_default, '') AS line
    FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name <> 'schema_migrations'
    UNION ALL
    SELECT 'index ' || indexname || ' ' || indexdef
    FROM pg_indexes
    WHERE schemaname = 'public' AND tablename <> 'schema_migrations'
    UNION ALL
    SELECT 'constraint ' || conname || ' ' || pg_get_constraintdef(oid)
    FROM pg_constraint
    WHERE connamespace = 'public'::regnamespace AND conrelid::regclass::text <> 'schema_migrations'
) described
