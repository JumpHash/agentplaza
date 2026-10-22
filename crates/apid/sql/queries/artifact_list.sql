-- Every artifact with its newest version's title, kind, number and size, and its author, project, update time and
-- shared flag: newest update first, then newest creation. No parameters.
SELECT "a"."id", "v"."title", "v"."kind", "v"."version", "a"."author", "a"."project", "a"."updated_ms", "v"."bytes", "a"."shared"
FROM "artifact" "a"
JOIN "artifact_version" "v" ON "v"."artifact_id" = "a"."id"
WHERE "v"."version" = (SELECT MAX("version") FROM "artifact_version" WHERE "artifact_id" = "a"."id")
ORDER BY "a"."updated_ms" DESC, "a"."pos" DESC
