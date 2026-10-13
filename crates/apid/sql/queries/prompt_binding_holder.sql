-- The id of a live prompt other than $2 that holds binding $1, if any. $1 = binding, $2 = the prompt's own id
SELECT id
FROM prompts
WHERE binding = $1 AND archived_at IS NULL AND id <> $2
LIMIT 1
