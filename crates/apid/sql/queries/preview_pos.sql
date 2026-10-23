-- The place of preview ?1 in the preview table: its own when it is stored, else the next after the highest (0 for the first).
-- ?1 = preview id
SELECT COALESCE((SELECT "pos" FROM "preview" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "preview"))
