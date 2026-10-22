-- The newest id of the prompt history (0 when it is empty); the next event takes this + 1. No parameters.
SELECT COALESCE(MAX("id"), 0) FROM "prompt_event"
