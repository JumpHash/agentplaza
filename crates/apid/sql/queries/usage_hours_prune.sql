-- Drops the hour rows that start before ?1 (unix ms).
DELETE FROM "usage_hour" WHERE "hour_ms" < ?1
