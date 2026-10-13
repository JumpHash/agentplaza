-- Creates the table that records applied migrations, if it does not exist. Runs under the
-- migration advisory lock, before any migration file is read. No parameters.
CREATE TABLE IF NOT EXISTS schema_migrations (
    version int PRIMARY KEY,
    name text NOT NULL,
    checksum text NOT NULL,
    applied_at timestamptz NOT NULL DEFAULT now()
)
