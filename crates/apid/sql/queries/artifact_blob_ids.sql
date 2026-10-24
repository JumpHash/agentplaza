-- The distinct data blob ids the versions of artifact ?1 reference.
-- ?1 = artifact id
SELECT DISTINCT "value" FROM "artifact_version_blobs" WHERE "artifact_version_artifact_id" = ?1
