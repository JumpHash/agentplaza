-- The call log, newest first. $1 = token id (NULL for any token), $2 = tool (NULL for any tool), $3 = limit (bigint).
SELECT token_id, tool, args_sha256, ok, error, duration_ms, round(extract(epoch FROM at) * 1000)::bigint, client_ip
FROM hub_mcp_calls
WHERE ($1::text IS NULL OR token_id = $1) AND ($2::text IS NULL OR tool = $2)
ORDER BY at DESC, id DESC
LIMIT $3
