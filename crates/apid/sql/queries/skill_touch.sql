-- Marks a skill updated. $1 = skill id, $2 = now_ms (bigint)
UPDATE skills
SET updated = to_timestamp(($2::bigint / 1000.0)::double precision)
WHERE id = $1
