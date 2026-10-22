-- The context split of one model call, in order: ?1 call id.
SELECT part, tokens FROM model_call_context WHERE call = ?1 ORDER BY pos;
