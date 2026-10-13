-- Replaces the title and tags. $1 = skill id, $2 = title, $3 = tags (text[]), $4 = now_ms (bigint)
UPDATE skills
SET title = $2,
    tags = $3,
    updated = to_timestamp(($4::bigint / 1000.0)::double precision)
WHERE id = $1
