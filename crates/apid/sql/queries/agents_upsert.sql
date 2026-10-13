-- Writes one agent's durable fields (the roster's write-through): inserts the row, or updates the row
-- with the same cid and makes it active again. $1 = cid (bigint), $2 = name (text), $3 = role (text),
-- $4 = model (text or null), $5 = species (text)
INSERT INTO agents (cid, name, role, model, species, state, updated_at)
VALUES ($1, $2, $3, $4, $5, 'active', now())
ON CONFLICT (cid) DO UPDATE SET name = EXCLUDED.name, role = EXCLUDED.role, model = EXCLUDED.model,
    species = EXCLUDED.species, state = 'active', retired_at = NULL, updated_at = now()
