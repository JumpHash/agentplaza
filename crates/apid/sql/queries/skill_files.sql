-- The files of one version, by path. $1 = skill id, $2 = version
SELECT path, encode(sha256, 'hex'), size, executable
FROM skill_files
WHERE skill_id = $1 AND version = $2
ORDER BY path
