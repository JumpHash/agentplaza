-- The hash of the compiled JavaScript of script ?4 of panel ?3 at version ?2 of artifact ?1 (no row when there is none).
-- ?1 = artifact id, ?2 = version, ?3 = panel id, ?4 = script path
SELECT "hash" FROM "artifact_version_scripts" WHERE "artifact_version_artifact_id" = ?1 AND "artifact_version_version" = ?2 AND "panel" = ?3 AND "path" = ?4
