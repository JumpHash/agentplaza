-- One skill with its timestamps in ms and its sorted bindings (the columns of skill_list.sql). $1 = skill id
SELECT s.id, s.name, s.title, s.description, s.tags, s.active_version, s.latest_version,
       s.source_kind, s.source_repo, s.source_ref, s.source_path, s.source_commit,
       (extract(epoch from s.created) * 1000)::bigint,
       (extract(epoch from s.updated) * 1000)::bigint,
       (extract(epoch from s.archived_at) * 1000)::bigint,
       COALESCE((SELECT array_agg(b.binding ORDER BY b.binding) FROM skill_bindings b WHERE b.skill_id = s.id), '{}')
FROM skills s
WHERE s.id = $1
