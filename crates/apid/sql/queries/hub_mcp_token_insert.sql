-- Inserts one token record. Only the sha256 (hex) of the token is stored, as bytes.
-- $1 = id, $2 = name, $3 = scope, $4 = allowlist (text[]), $5 = token sha256 (hex),
-- $6 = created_ms (bigint), $7 = expires_ms (bigint, NULL when the token never expires)
INSERT INTO hub_mcp_tokens (id, name, scope, allowlist, token_sha256, created, expires)
VALUES ($1, $2, $3, $4, decode($5, 'hex'),
        to_timestamp(($6::bigint / 1000.0)::double precision),
        to_timestamp(($7::bigint / 1000.0)::double precision))
