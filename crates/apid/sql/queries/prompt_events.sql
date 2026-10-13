-- The history of a prompt, newest first, at most $2 rows. $1 = prompt id, $2 = limit (bigint)
SELECT e.id, e.prompt_id, e.action, e.version, e.actor,
       (extract(epoch from e."at") * 1000)::bigint
FROM prompt_events e
WHERE e.prompt_id = $1
ORDER BY e.id DESC
LIMIT $2
