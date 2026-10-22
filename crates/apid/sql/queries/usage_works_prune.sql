-- Keeps the newest ?1 work rows by work id: drops the rows whose work id is below the ?1th largest.
DELETE FROM "usage_work"
WHERE "work" < (SELECT "work" FROM "usage_work" ORDER BY "work" DESC LIMIT 1 OFFSET ?1)
