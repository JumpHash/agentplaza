-- The versions of a prompt, oldest first, without bodies. $1 = prompt id
SELECT version, note, author,
       (extract(epoch from created) * 1000)::bigint,
       score, errors, warnings, infos, rules_version,
       encode(body_sha256, 'hex')
FROM prompt_versions
WHERE prompt_id = $1
ORDER BY version
