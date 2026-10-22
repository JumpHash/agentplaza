-- Writes one agent's durable fields (the roster's write-through): inserts the row, or updates the row
-- with the same cid and makes it active again. $1 = cid (bigint), $2 = role (text), $3 = model (text or null).
-- The name column (NOT NULL, migration 0003) no longer holds a name: a new row takes ''.
INSERT INTO agents (cid, name, role, model, state, updated_at)
VALUES ($1, '', $2, $3, 'active', now())
ON CONFLICT (cid) DO UPDATE SET role = EXCLUDED.role, model = EXCLUDED.model,
    state = 'active', retired_at = NULL, updated_at = now()
