-- The agents per role, active and retired rows alike: the queryable check. No parameters.
SELECT role, count(*) FROM agents GROUP BY role ORDER BY role
