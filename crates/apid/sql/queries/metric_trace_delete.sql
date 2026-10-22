-- Deletes one kept trace (its spans and attributes go with it): ?1 trace sequence.
DELETE FROM trace WHERE seq = ?1;
