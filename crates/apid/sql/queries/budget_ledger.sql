-- A budget's ledger, newest first: ?1 owner_kind ('project' | 'task'), ?2 owner, ?3 the most entries.
SELECT "owner_kind", "owner", "kind", "amount", "micros", "tokens", "session", "model", "at_ms"
FROM "budget_entry"
WHERE "owner_kind" = ?1 AND "owner" = ?2
ORDER BY "pos" DESC
LIMIT ?3
