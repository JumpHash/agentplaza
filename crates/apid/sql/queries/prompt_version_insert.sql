-- Appends an immutable version with its check score. $1 = prompt id, $2 = version, $3 = body, $4 = sha256 (lower-case hex),
-- $5 = note, $6 = author, $7 = score, $8 = errors, $9 = warnings, $10 = infos, $11 = rules_version, $12 = now_ms (bigint)
INSERT INTO prompt_versions (prompt_id, version, body, body_sha256, note, author,
                             score, errors, warnings, infos, rules_version, created)
VALUES ($1, $2, $3, decode($4, 'hex'), $5, $6, $7, $8, $9, $10, $11,
        to_timestamp(($12::bigint / 1000.0)::double precision))
