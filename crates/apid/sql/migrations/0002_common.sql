-- 0002 common: the data migration's bookkeeping (data_migrations, one row per domain) and the
-- forbid_mutation() trigger function the append-only tables use (docs/design/database.md 2.0, 2.6, 4.1).
CREATE TABLE data_migrations (
    domain text PRIMARY KEY,
    status text NOT NULL CHECK (status IN ('pending', 'running', 'verified', 'failed')),
    source text NOT NULL CHECK (source IN ('blob', 'file', 'none')),
    source_sha256 text,
    rows_expected bigint NOT NULL DEFAULT 0 CHECK (rows_expected >= 0),
    rows_loaded bigint NOT NULL DEFAULT 0 CHECK (rows_loaded >= 0),
    detail text,
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);

-- Raises on every UPDATE, DELETE and TRUNCATE of the table it is attached to. Attach it as a
-- BEFORE UPDATE OR DELETE row trigger and a BEFORE TRUNCATE statement trigger. It applies to every role.
CREATE FUNCTION forbid_mutation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'append-only: % on % is not allowed', TG_OP, TG_TABLE_NAME;
    RETURN NULL;
END;
$$;
