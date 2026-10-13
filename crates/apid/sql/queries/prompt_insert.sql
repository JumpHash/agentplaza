-- Creates a prompt, unpublished, at latest version 1. $1 = id, $2 = kind, $3 = title, $4 = description,
-- $5 = tags (text[]), $6 = binding (or null), $7 = now_ms (bigint)
INSERT INTO prompts (id, kind, title, description, tags, binding, created, updated)
VALUES ($1, $2, $3, $4, $5, $6,
        to_timestamp(($7::bigint / 1000.0)::double precision),
        to_timestamp(($7::bigint / 1000.0)::double precision))
