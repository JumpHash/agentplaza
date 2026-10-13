-- The id of a live skill other than $2 that holds the name $1, if any. $1 = name, $2 = the skill's own id
SELECT id
FROM skills
WHERE archived_at IS NULL AND name = $1 AND id <> $2
LIMIT 1
