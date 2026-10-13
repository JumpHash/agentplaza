-- One version: the metadata columns of skill_versions.sql, then SKILL.md last. $1 = skill id, $2 = version
SELECT version, note, author,
       (extract(epoch from created) * 1000)::bigint,
       score, errors, warnings, infos, rules_version,
       encode(sha256, 'hex'), file_count, total_bytes, source_commit,
       skill_md
FROM skill_versions
WHERE skill_id = $1 AND version = $2
