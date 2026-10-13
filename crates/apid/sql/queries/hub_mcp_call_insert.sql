-- Appends one call to the audit log (append-only: no UPDATE or DELETE).
-- $1 = token_id (NULL for an anonymous call), $2 = client_ip (NULL for a token's call), $3 = tool, $4 = args_sha256 (hex),
-- $5 = ok, $6 = error (NULL when none), $7 = duration_ms (bigint), $8 = at_ms (bigint)
INSERT INTO hub_mcp_calls (token_id, client_ip, tool, args_sha256, ok, error, duration_ms, at)
VALUES ($1, $2, $3, $4, $5, $6, $7, to_timestamp(($8::bigint / 1000.0)::double precision))
