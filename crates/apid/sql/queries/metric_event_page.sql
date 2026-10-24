-- One page of events at times in [?1, ?2), newest first: ?3 the most rows, ?4 the rows to skip.
SELECT id, t_ms, kind, entity_kind, entity, duration_ms, detail FROM metric_event
WHERE t_ms >= ?1 AND t_ms < ?2
ORDER BY t_ms DESC, id DESC
LIMIT ?3 OFFSET ?4;
