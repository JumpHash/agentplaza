-- The versions the artifacts of author ?1 (NULL for the owner) got since ?2 (ms): their count and their spec bytes.
-- ?1 = author cid (NULL for the owner), ?2 = since (ms)
SELECT COUNT(*), COALESCE(SUM("v"."spec_bytes"), 0)
FROM "artifact_version" "v"
JOIN "artifact" "a" ON "a"."id" = "v"."artifact_id"
WHERE "a"."author_cid" IS ?1 AND "v"."created_ms" >= ?2
