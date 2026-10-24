-- The bindings of skill ?1 (role:<role> or plaza), sorted.
-- ?1 = skill id
SELECT "value" FROM "skill_bindings" WHERE "skill_id" = ?1 ORDER BY "bindings_pos"
