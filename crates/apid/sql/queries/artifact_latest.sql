-- The newest version number of artifact ?1 (0 when it has none).
-- ?1 = artifact id
SELECT COALESCE(MAX("version"), 0) FROM "artifact_version" WHERE "artifact_id" = ?1
