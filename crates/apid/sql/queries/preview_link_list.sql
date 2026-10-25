-- The links of preview ?1, newest first.
-- ?1 = preview id
SELECT * FROM "preview_link" WHERE "preview" = ?1 ORDER BY "pos" DESC
