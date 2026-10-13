-- The history of a skill, newest first, at most $2 rows. $1 = skill id, $2 = limit (bigint)
SELECT e.id, e.skill_id, e.action, e.version, e.actor,
       (extract(epoch from e."at") * 1000)::bigint
FROM skill_events e
WHERE e.skill_id = $1
ORDER BY e.id DESC
LIMIT $2
