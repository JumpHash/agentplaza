-- Replaces the editable metadata. $1 = prompt id, $2 = title, $3 = description, $4 = tags (text[]),
-- $5 = binding (or null), $6 = now_ms (bigint)
UPDATE prompts
SET title = $2,
    description = $3,
    tags = $4,
    binding = $5,
    updated = to_timestamp(($6::bigint / 1000.0)::double precision)
WHERE id = $1
