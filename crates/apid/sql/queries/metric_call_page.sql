-- One page of model calls made before ?1 (ms), newest first: ?2 the most rows, ?3 the rows to skip.
SELECT id, at_ms, cid, role, run, node, provider, model, engine, tokens_in, cache_read, cache_write, tokens_out, cost_usd, ttft_ms, duration_ms, status, retries, task, trace_id
FROM model_call
WHERE at_ms < ?1
ORDER BY at_ms DESC, id DESC
LIMIT ?2 OFFSET ?3;
