-- Appends one check finding of a version. $1 = skill id, $2 = version, $3 = seq, $4 = rule, $5 = severity,
-- $6 = path, $7 = line, $8 = message
INSERT INTO skill_findings (skill_id, version, seq, rule, severity, path, line, message)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
