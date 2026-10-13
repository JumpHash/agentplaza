-- Archives a skill (keeping its first archive time) or restores it (clears the time). $1 = skill id,
-- $2 = archived (boolean), $3 = now_ms (bigint)
UPDATE skills
SET archived_at = CASE WHEN $2::boolean
                       THEN COALESCE(archived_at, to_timestamp(($3::bigint / 1000.0)::double precision))
                  END,
    updated = to_timestamp(($3::bigint / 1000.0)::double precision)
WHERE id = $1
