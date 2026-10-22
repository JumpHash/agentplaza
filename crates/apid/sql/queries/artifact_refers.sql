-- How many version rows of artifact ?1 reference data blob ?2.
-- ?1 = artifact id, ?2 = blob id
SELECT COUNT(*) FROM "artifact_version_blobs" WHERE "artifact_version_artifact_id" = ?1 AND "value" = ?2
