-- The versions of artifact ?1, oldest first: number, creation time and title (the spec is not read).
-- ?1 = artifact id
SELECT "version", "created_ms", "title" FROM "artifact_version" WHERE "artifact_id" = ?1 ORDER BY "version"
