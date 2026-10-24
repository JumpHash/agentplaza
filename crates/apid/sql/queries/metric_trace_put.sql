-- Puts one kept trace (its sequence number is assigned): ?1 trace id, ?2 cid, ?3 role, ?4 start ms, ?5 duration ms, ?6 ok, ?7 why kept (NULL when none), ?8 bytes it takes.
INSERT INTO trace (id, cid, role, start_ms, dur_ms, ok, kept, bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8);
