-- Puts one span of a trace: ?1 trace sequence, ?2 position, ?3 span id, ?4 parent id (NULL when none), ?5 kind, ?6 name, ?7 start ms, ?8 duration ms, ?9 time to first token ms (NULL when none), ?10 ok.
INSERT INTO trace_span (trace, pos, span_id, parent, kind, name, start_ms, dur_ms, first_token_ms, ok)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10);
