-- Reads the token record whose sha256 is the given digest (no row when none matches). Times are ms.
-- $1 = token sha256 (hex)
SELECT id, name, scope, allowlist,
       round(extract(epoch FROM created) * 1000)::bigint,
       round(extract(epoch FROM expires) * 1000)::bigint,
       round(extract(epoch FROM last_used) * 1000)::bigint,
       calls,
       round(extract(epoch FROM revoked_at) * 1000)::bigint
FROM hub_mcp_tokens WHERE token_sha256 = decode($1, 'hex')
