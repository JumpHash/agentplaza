-- Version ?2 of prompt ?1 with its body, if there is such a version. Its findings come from prompt_findings.sql.
-- ?1 = prompt id, ?2 = version
SELECT * FROM "prompt_version" WHERE "prompt_id" = ?1 AND "version" = ?2
