-- Stores a component's snapshot, replacing the previous one; sets updated_at to now.
-- $1 = component (text), $2 = snapshot JSON (text, cast to jsonb), $3 = FNV-1a hash of $2 (bigint).
INSERT INTO plaza_state (component, data, hash, updated_at)
VALUES ($1, $2::text::jsonb, $3, now())
ON CONFLICT (component) DO UPDATE SET data = EXCLUDED.data, hash = EXCLUDED.hash, updated_at = now()
