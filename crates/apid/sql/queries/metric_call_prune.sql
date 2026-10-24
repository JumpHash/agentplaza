-- Deletes the model calls before ?1 (ms); their context goes with them.
DELETE FROM model_call WHERE at_ms < ?1;
