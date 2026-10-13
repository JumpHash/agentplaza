-- Appends one check finding of a version. $1 = prompt id, $2 = version, $3 = seq, $4 = rule, $5 = severity,
-- $6 = line, $7 = message
INSERT INTO prompt_findings (prompt_id, version, seq, rule, severity, line, message)
VALUES ($1, $2, $3, $4, $5, $6, $7)
