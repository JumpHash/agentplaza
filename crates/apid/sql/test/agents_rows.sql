-- Test only: the agents table as one string, a row per agent: cid|name|role|model|species|state|created_ms, by cid.
SELECT string_agg(cid || '|' || name || '|' || role || '|' || coalesce(model, '-') || '|' || species || '|' || state || '|' || (extract(epoch from created_at) * 1000)::bigint, ';' ORDER BY cid) FROM agents
