-- Retires one agent (the roster's remove): a soft delete, the row and its history stay. Only an active
-- row changes. $1 = cid (bigint)
UPDATE agents SET state = 'retired', retired_at = now(), updated_at = now() WHERE cid = $1 AND state = 'active'
