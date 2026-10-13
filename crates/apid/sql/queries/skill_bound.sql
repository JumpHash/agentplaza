-- The live, published skills holding a binding, by name: the skill_get.sql columns, then the live version's
-- metadata columns (from version, at column 16). $1 = binding (role:<role> or plaza)
SELECT s.id, s.name, s.title, s.description, s.tags, s.active_version, s.latest_version,
       s.source_kind, s.source_repo, s.source_ref, s.source_path, s.source_commit,
       (extract(epoch from s.created) * 1000)::bigint,
       (extract(epoch from s.updated) * 1000)::bigint,
       (extract(epoch from s.archived_at) * 1000)::bigint,
       COALESCE((SELECT array_agg(b.binding ORDER BY b.binding) FROM skill_bindings b WHERE b.skill_id = s.id), '{}'),
       v.version, v.note, v.author,
       (extract(epoch from v.created) * 1000)::bigint,
       v.score, v.errors, v.warnings, v.infos, v.rules_version,
       encode(v.sha256, 'hex'), v.file_count, v.total_bytes, v.source_commit
FROM skills s
JOIN skill_versions v ON v.skill_id = s.id AND v.version = s.active_version
WHERE s.archived_at IS NULL
  AND EXISTS (SELECT 1 FROM skill_bindings x WHERE x.skill_id = s.id AND x.binding = $1)
ORDER BY s.name
