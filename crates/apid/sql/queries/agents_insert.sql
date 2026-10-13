-- Inserts one agent from the blob. The *_ms values are epoch milliseconds; a missing created_at or
-- updated_at takes the load time. $1 = cid (bigint), $2 = name, $3 = role, $4 = model (text or null),
-- $5 = species (text), $6 = species_at_ms (bigint or null), $7 = state (active or retired),
-- $8 = created_at_ms (bigint or null), $9 = retired_at_ms (bigint or null), $10 = updated_at_ms (bigint or null)
INSERT INTO agents (cid, name, role, model, species, species_at, state, created_at, retired_at, updated_at)
VALUES ($1, $2, $3, $4, $5,
        to_timestamp(($6::bigint)::float8 / 1000.0),
        $7,
        COALESCE(to_timestamp(($8::bigint)::float8 / 1000.0), now()),
        to_timestamp(($9::bigint)::float8 / 1000.0),
        COALESCE(to_timestamp(($10::bigint)::float8 / 1000.0), now()))
