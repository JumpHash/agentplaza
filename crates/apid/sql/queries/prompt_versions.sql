-- The metadata of every version of prompt ?1, oldest first. No bodies: the store reads a body only when asked for one.
-- ?1 = prompt id
SELECT "version", "note", "author", "created_ms", "score", "errors", "warnings", "infos", "rules_version", "sha256"
FROM "prompt_version" WHERE "prompt_id" = ?1 ORDER BY "version"
