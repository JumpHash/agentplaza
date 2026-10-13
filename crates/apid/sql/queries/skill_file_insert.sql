-- Appends one file of a version. $1 = skill id, $2 = version, $3 = path, $4 = sha256 (lower-case hex),
-- $5 = size (bigint), $6 = executable
INSERT INTO skill_files (skill_id, version, path, sha256, size, executable)
VALUES ($1, $2, $3, decode($4, 'hex'), $5, $6)
