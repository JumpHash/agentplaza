-- Deletes every stored entry of one agent (a wipe). ?1 = cid.
DELETE FROM "entry" WHERE "cid" = ?1
