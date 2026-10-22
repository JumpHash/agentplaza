-- Drops the calls older than the newest ?1 (the rows below the ?1th newest id). A table with fewer rows keeps all.
DELETE FROM "usage_call"
WHERE "id" < (SELECT "id" FROM "usage_call" ORDER BY "id" DESC LIMIT 1 OFFSET ?1)
