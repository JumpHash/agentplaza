-- The place of share ?1 in the share table: its own when it is stored, else the next after the highest (0 for the first).
-- ?1 = share id
SELECT COALESCE((SELECT "pos" FROM "share" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "share"))
