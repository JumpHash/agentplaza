-- The bytes all kept traces take together.
SELECT COALESCE(SUM(bytes), 0) FROM trace;
