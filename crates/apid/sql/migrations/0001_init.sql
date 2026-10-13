-- 0001 init: the plaza's first schema, the tables the apid db module created before migrations.
-- Every statement is IF NOT EXISTS, so a database that already has these tables records this
-- migration without changing them.
CREATE TABLE IF NOT EXISTS plaza_state (
    component text PRIMARY KEY,
    data jsonb NOT NULL,
    hash bigint NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS transcript (
    cid bigint,
    seq bigint,
    work bigint,
    kind text,
    at_ms bigint,
    data jsonb,
    PRIMARY KEY (cid, seq)
);
CREATE TABLE IF NOT EXISTS infraction (
    id bigserial PRIMARY KEY,
    cid bigint NOT NULL,
    law text NOT NULL,
    at_ms bigint NOT NULL,
    tool text NOT NULL,
    detail text NOT NULL
);
CREATE INDEX IF NOT EXISTS infraction_cid ON infraction (cid, at_ms DESC);
