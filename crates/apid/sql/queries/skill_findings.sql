-- The check findings of one version, in the order the check made them. $1 = skill id, $2 = version
SELECT rule, severity, path, line, message
FROM skill_findings
WHERE skill_id = $1 AND version = $2
ORDER BY seq
