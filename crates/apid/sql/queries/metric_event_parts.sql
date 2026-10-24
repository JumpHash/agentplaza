-- The named numbers of one event, in the order they were given: ?1 event id.
SELECT name, value FROM metric_event_part WHERE event = ?1 ORDER BY pos;
