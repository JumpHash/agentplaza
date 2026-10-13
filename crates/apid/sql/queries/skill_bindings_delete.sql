-- Removes every binding of a skill, before its new ones are inserted. $1 = skill id
DELETE FROM skill_bindings
WHERE skill_id = $1
