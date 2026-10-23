-- The compiled scripts of version ?2 of artifact ?1, in their order, every column in table order.
-- ?1 = artifact id, ?2 = version
SELECT * FROM "artifact_version_scripts" WHERE "artifact_version_artifact_id" = ?1 AND "artifact_version_version" = ?2 ORDER BY "scripts_pos"
