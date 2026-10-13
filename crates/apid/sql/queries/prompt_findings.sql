-- The check findings of one version, in the order the check made them. $1 = prompt id, $2 = version
SELECT rule, severity, line, message
FROM prompt_findings
WHERE prompt_id = $1 AND version = $2
ORDER BY seq
