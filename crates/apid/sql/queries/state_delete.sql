-- Removes one component's stored snapshot. Used by tests only (cleanup of a test component).
-- $1 = component (text)
DELETE FROM plaza_state WHERE component = $1
