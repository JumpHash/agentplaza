-- The newest seq of the share event log (0 when it is empty); the next event takes this + 1. No parameters.
SELECT COALESCE(MAX("seq"), 0) FROM "share_event"
