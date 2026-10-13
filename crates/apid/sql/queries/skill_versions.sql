-- The versions of a skill, oldest first, without SKILL.md. $1 = skill id
SELECT version, note, author,
       (extract(epoch from created) * 1000)::bigint,
       score, errors, warnings, infos, rules_version,
       encode(sha256, 'hex'), file_count, total_bytes, source_commit
FROM skill_versions
WHERE skill_id = $1
ORDER BY version
