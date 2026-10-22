-- The place of blob ?1 in the blob table: the one it has, else the next after the highest (0 for the first).
-- ?1 = blob id
SELECT COALESCE((SELECT "pos" FROM "artifact_blob" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "artifact_blob"))
