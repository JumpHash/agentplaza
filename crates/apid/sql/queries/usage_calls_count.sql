-- How many retained calls match: ?1 model (NULL for any), ?2 cid (NULL for any).
SELECT COUNT(*) FROM "usage_call"
WHERE ("model" = ?1 OR ?1 IS NULL) AND ("cid" = ?2 OR ?2 IS NULL)
