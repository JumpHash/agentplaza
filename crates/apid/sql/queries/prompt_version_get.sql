-- One version with its body (the metadata columns first, then body). $1 = prompt id, $2 = version
SELECT version, note, author,
       (extract(epoch from created) * 1000)::bigint,
       score, errors, warnings, infos, rules_version,
       encode(body_sha256, 'hex'),
       body
FROM prompt_versions
WHERE prompt_id = $1 AND version = $2
