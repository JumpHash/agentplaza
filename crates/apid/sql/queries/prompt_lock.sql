-- The prompt row as prompt_get.sql reads it, locked until the transaction ends. $1 = prompt id
SELECT id, kind, title, description, tags, binding, active_version, latest_version,
       (extract(epoch from created) * 1000)::bigint,
       (extract(epoch from updated) * 1000)::bigint,
       (extract(epoch from archived_at) * 1000)::bigint
FROM prompts
WHERE id = $1
FOR UPDATE
