-- Appends an immutable version with its check score and totals. $1 = skill id, $2 = version, $3 = SKILL.md,
-- $4 = sha256 of SKILL.md (lower-case hex), $5 = note, $6 = author, $7 = score, $8 = errors, $9 = warnings,
-- $10 = infos, $11 = rules_version, $12 = file_count, $13 = total_bytes, $14 = source commit (or null), $15 = now_ms
INSERT INTO skill_versions (skill_id, version, skill_md, sha256, note, author, score, errors, warnings, infos,
                            rules_version, file_count, total_bytes, source_commit, created)
VALUES ($1, $2, $3, decode($4, 'hex'), $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
        to_timestamp(($15::bigint / 1000.0)::double precision))
