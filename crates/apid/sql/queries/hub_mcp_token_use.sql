-- Counts one call by a token and stamps its last use.
-- $1 = token id, $2 = now_ms (bigint)
UPDATE hub_mcp_tokens SET calls = calls + 1,
       last_used = to_timestamp(($2::bigint / 1000.0)::double precision)
WHERE id = $1
