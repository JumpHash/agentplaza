-- The place of link ?1 in the preview link table: its own when it is stored, else the next after the highest (0 for the first).
-- ?1 = link id
SELECT COALESCE((SELECT "pos" FROM "preview_link" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "preview_link"))
