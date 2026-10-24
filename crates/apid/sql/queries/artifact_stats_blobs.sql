-- The bytes of the data blobs the agent cid (NULL for the owner) attached since ?2 (ms).
-- ?1 = attaching cid (NULL for the owner), ?2 = since (ms)
SELECT COALESCE(SUM("bytes"), 0) FROM "artifact_blob" WHERE "cid" IS ?1 AND "created_ms" >= ?2
