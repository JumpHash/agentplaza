-- The ids of the data blobs version ?2 of artifact ?1 references, in their order.
-- ?1 = artifact id, ?2 = version
SELECT "value" FROM "artifact_version_blobs" WHERE "artifact_version_artifact_id" = ?1 AND "artifact_version_version" = ?2 ORDER BY "blobs_pos"
