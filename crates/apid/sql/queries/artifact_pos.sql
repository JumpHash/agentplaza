-- The place of artifact ?1 in the artifact table: the one it has, else the next after the highest (0 for the first).
-- ?1 = artifact id
SELECT COALESCE((SELECT "pos" FROM "artifact" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "artifact"))
