-- The vault audit's lines after a sequence number, in chain order: ?1 the sequence number, ?2 the most lines.
SELECT "seq", "ts_ms", "prev", "mac", "event", "actor", "entry", "name", "binding", "node", "cid", "detail"
FROM "vault_audit"
WHERE "seq" > ?1
ORDER BY "seq"
LIMIT ?2
