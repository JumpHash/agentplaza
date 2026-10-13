-- Every skill by id; archived ones only when $1 is true. $1 = include archived (boolean)
SELECT s.id, s.name, s.title, s.description, s.tags, s.active_version, s.latest_version,
       s.source_kind, s.source_repo, s.source_ref, s.source_path, s.source_commit,
       (extract(epoch from s.created) * 1000)::bigint,
       (extract(epoch from s.updated) * 1000)::bigint,
       (extract(epoch from s.archived_at) * 1000)::bigint,
       COALESCE((SELECT array_agg(b.binding ORDER BY b.binding) FROM skill_bindings b WHERE b.skill_id = s.id), '{}')
FROM skills s
WHERE ($1::boolean OR s.archived_at IS NULL)
ORDER BY s.id
