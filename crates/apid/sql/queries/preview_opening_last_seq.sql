-- The newest seq of the preview opening log (0 when it is empty); the next opening takes this + 1. No parameters.
SELECT COALESCE(MAX("seq"), 0) FROM "preview_opening"
