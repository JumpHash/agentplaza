-- Deletes every stored transcript record of one agent (a wipe).
-- $1 = cid (bigint)
DELETE FROM transcript WHERE cid = $1
