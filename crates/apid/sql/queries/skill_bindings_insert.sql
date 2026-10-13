-- Binds a skill to a binding. $1 = skill id, $2 = binding (plaza or role:<role>)
INSERT INTO skill_bindings (skill_id, binding)
VALUES ($1, $2)
