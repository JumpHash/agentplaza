-- Records the prompt's highest version. $1 = prompt id, $2 = latest version, $3 = now_ms (bigint)
UPDATE prompts
SET latest_version = $2,
    updated = to_timestamp(($3::bigint / 1000.0)::double precision)
WHERE id = $1
