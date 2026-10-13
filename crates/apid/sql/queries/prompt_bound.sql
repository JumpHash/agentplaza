-- The live prompt bound to a binding that has a published version, with that version's body (body last).
-- $1 = binding (role:<role> or plaza)
SELECT p.id, p.kind, p.title, p.description, p.tags, p.binding, p.active_version, p.latest_version,
       (extract(epoch from p.created) * 1000)::bigint,
       (extract(epoch from p.updated) * 1000)::bigint,
       (extract(epoch from p.archived_at) * 1000)::bigint,
       v.body
FROM prompts p
JOIN prompt_versions v ON v.prompt_id = p.id AND v.version = p.active_version
WHERE p.binding = $1 AND p.archived_at IS NULL
