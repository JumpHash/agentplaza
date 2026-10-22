-- The ID of the token whose digest is the given one (no row when none matches). ?1 = the token's sha256 (bytes).
SELECT "token_id" FROM "hub_mcp_token_digest" WHERE "digest" = ?1
