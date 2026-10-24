-- The attributes of one trace's spans, by span and in order: ?1 trace sequence.
SELECT span, name, value FROM trace_span_attr WHERE trace = ?1 ORDER BY span, pos;
