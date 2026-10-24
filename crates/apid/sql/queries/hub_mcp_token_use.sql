-- Counts one call by a token and stamps its last use. ?1 = the token's id, ?2 = now in ms.
UPDATE "hub_mcp_token" SET "calls" = "calls" + 1, "last_used_ms" = ?2 WHERE "id" = ?1
