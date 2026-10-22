-- Version ?2 of artifact ?1, every column in table order (its blobs and scripts are read by their own queries).
-- ?1 = artifact id, ?2 = version
SELECT * FROM "artifact_version" WHERE "artifact_id" = ?1 AND "version" = ?2
