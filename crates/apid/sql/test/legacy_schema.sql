-- Test only: the schema the apid db module ran on connect before the migration runner existed
-- (verbatim). A database created with it must migrate cleanly and record 0001 as applied.

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
