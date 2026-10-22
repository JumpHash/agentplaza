-- The newest owner actions of one workspace, newest first (served by ide_audit_workspace_at_ms). ?1 = workspace,
-- ?2 = the most rows.
SELECT * FROM "ide_audit" WHERE "workspace" = ?1 ORDER BY "at_ms" DESC, "seq" DESC LIMIT ?2
