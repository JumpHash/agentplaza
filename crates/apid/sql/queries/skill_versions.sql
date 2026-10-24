-- The metadata of every version of skill ?1, oldest first. No SKILL.md: the store reads one version's text, files and
-- findings only when asked for that version.
-- ?1 = skill id
SELECT "version", "note", "author", "created_ms", "score", "errors", "warnings", "infos", "rules_version", "hash",
       "file_count", "total_bytes", "source_commit"
FROM "skill_version" WHERE "skill_id" = ?1 ORDER BY "version"
