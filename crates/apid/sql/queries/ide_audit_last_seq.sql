-- The newest seq of the IDE audit (0 when it is empty); the next entry takes this + 1. No parameters.
SELECT COALESCE(MAX("seq"), 0) FROM "ide_audit"
