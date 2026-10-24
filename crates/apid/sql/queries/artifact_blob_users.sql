-- How many version rows (of any artifact) reference data blob ?1.
-- ?1 = blob id
SELECT COUNT(*) FROM "artifact_version_blobs" WHERE "value" = ?1
