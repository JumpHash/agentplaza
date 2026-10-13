-- The active agents, by cid, for the roster at start. Retired rows stay in the table and are not
-- loaded. No parameters.
SELECT cid, name, role, model, species FROM agents WHERE state = 'active' ORDER BY cid
