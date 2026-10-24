-- Removes every version of artifact ?1 (their blob and script rows go with them, by cascade).
-- ?1 = artifact id
DELETE FROM "artifact_version" WHERE "artifact_id" = ?1
