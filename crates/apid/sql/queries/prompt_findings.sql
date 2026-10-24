-- The check findings of version ?2 of prompt ?1, in the order the check made them.
-- ?1 = prompt id, ?2 = version
SELECT * FROM "prompt_version_findings" WHERE "prompt_version_prompt_id" = ?1 AND "prompt_version_version" = ?2 ORDER BY "findings_pos"
