-- The shares that are neither revoked nor expired at ?1 (Unix ms), newest first. An expiry at or before ?1 has passed.
-- ?1 = now, in Unix ms
SELECT * FROM "share" WHERE "revoked_ms" IS NULL AND ("expires_ms" IS NULL OR "expires_ms" > ?1) ORDER BY "pos" DESC
