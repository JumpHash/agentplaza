-- The newest owner actions of one workspace, newest first (served by ide_audit_workspace_at).
-- $1 = workspace (text), $2 = limit (bigint)
SELECT data FROM ide_audit WHERE workspace = $1 ORDER BY at_ms DESC, seq DESC LIMIT $2
