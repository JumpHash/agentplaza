-- Records a new latest version: its name and description follow its frontmatter, and a GitHub skill's commit
-- follows the version when it has one. $1 = skill id, $2 = latest version, $3 = name, $4 = description,
-- $5 = source commit (or null), $6 = now_ms (bigint)
UPDATE skills
SET latest_version = $2,
    name = $3,
    description = $4,
    source_commit = COALESCE($5, source_commit),
    updated = to_timestamp(($6::bigint / 1000.0)::double precision)
WHERE id = $1
