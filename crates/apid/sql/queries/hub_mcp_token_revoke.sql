-- Revokes a live token (no row changes when it is unknown or already revoked). ?1 = the token's id, ?2 = now in ms.
UPDATE "hub_mcp_token" SET "revoked_at_ms" = ?2 WHERE "id" = ?1 AND "revoked_at_ms" IS NULL
