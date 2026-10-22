-- The spans of one trace, in order: ?1 trace sequence.
SELECT pos, span_id, parent, kind, name, start_ms, dur_ms, first_token_ms, ok FROM trace_span WHERE trace = ?1 ORDER BY pos;
