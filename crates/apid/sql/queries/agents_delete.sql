-- Empties agents before the data step loads the blob. Runs in the step's transaction, so a failed
-- load rolls back to the table as it was. No parameters.
DELETE FROM agents
