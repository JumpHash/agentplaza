-- The metadata of version ?2 of skill ?1, if there is such a version (the columns of skill_versions.sql).
-- ?1 = skill id, ?2 = version
SELECT "version", "note", "author", "created_ms", "score", "errors", "warnings", "infos", "rules_version", "hash",
       "file_count", "total_bytes", "source_commit"
FROM "skill_version" WHERE "skill_id" = ?1 AND "version" = ?2
