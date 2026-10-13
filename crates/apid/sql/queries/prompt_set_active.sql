-- Sets the live version (null unpublishes). $1 = prompt id, $2 = version (or null), $3 = now_ms (bigint)
UPDATE prompts
SET active_version = $2,
    updated = to_timestamp(($3::bigint / 1000.0)::double precision)
WHERE id = $1
