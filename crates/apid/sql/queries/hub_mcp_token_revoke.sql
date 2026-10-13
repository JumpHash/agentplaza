-- Revokes a live token (no row when it is unknown or already revoked).
-- $1 = token id, $2 = now_ms (bigint)
UPDATE hub_mcp_tokens SET revoked_at = to_timestamp(($2::bigint / 1000.0)::double precision)
WHERE id = $1 AND revoked_at IS NULL
