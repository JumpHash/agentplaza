-- Puts one attribute of a span: ?1 trace sequence, ?2 span position, ?3 position, ?4 name, ?5 value.
INSERT INTO trace_span_attr (trace, span, pos, name, value) VALUES (?1, ?2, ?3, ?4, ?5);
