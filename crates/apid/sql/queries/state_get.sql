-- Reads one component's stored snapshot as text (no row when the component was never stored).
-- $1 = component (text)
SELECT data::text FROM plaza_state WHERE component = $1
