-- Every token record, newest first. Times are ms. The digests and the tokens are never read. No parameters.
SELECT id, name, scope, allowlist,
       round(extract(epoch FROM created) * 1000)::bigint,
       round(extract(epoch FROM expires) * 1000)::bigint,
       round(extract(epoch FROM last_used) * 1000)::bigint,
       calls,
       round(extract(epoch FROM revoked_at) * 1000)::bigint
FROM hub_mcp_tokens ORDER BY created DESC, id DESC
