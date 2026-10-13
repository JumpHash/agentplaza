-- Every prompt by id; archived ones only when $1 is true. $1 = include archived (boolean)
SELECT id, kind, title, description, tags, binding, active_version, latest_version,
       (extract(epoch from created) * 1000)::bigint,
       (extract(epoch from updated) * 1000)::bigint,
       (extract(epoch from archived_at) * 1000)::bigint
FROM prompts
WHERE ($1::boolean OR archived_at IS NULL)
ORDER BY id
