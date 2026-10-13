-- Creates a skill, unpublished, at latest version 1. $1 = id, $2 = name, $3 = title, $4 = description,
-- $5 = tags (text[]), $6 = source kind, $7 = source repo (or null), $8 = source ref (or null),
-- $9 = source path (or null), $10 = source commit (or null), $11 = now_ms (bigint)
INSERT INTO skills (id, name, title, description, tags, source_kind, source_repo, source_ref, source_path, source_commit,
                    created, updated)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
        to_timestamp(($11::bigint / 1000.0)::double precision),
        to_timestamp(($11::bigint / 1000.0)::double precision))
