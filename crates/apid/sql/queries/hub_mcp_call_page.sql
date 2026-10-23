-- The call log, newest first (by seq). ?1 = the token's id (NULL for any token), ?2 = the tool (NULL for any tool),
-- ?3 = the most rows.
SELECT * FROM "hub_mcp_call"
WHERE (?1 IS NULL OR "call_token_id" = ?1) AND (?2 IS NULL OR "call_tool" = ?2)
ORDER BY "seq" DESC
LIMIT ?3
