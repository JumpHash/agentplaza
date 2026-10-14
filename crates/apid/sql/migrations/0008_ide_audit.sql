-- 0008 ide_audit: the owner's IDE actions on agent workspaces (crates/apid/src/ide.rs): a write, a restore, a run
-- and its stop. One row per action; data is the whole row the route returns (at_ms, actor, workspace, kind, path and
-- the action's own fields), and the columns repeat the ones the route filters and orders by. Append-only.
CREATE TABLE ide_audit (
    seq bigserial PRIMARY KEY,
    at_ms bigint NOT NULL,
    workspace text NOT NULL,
    kind text NOT NULL,
    data jsonb NOT NULL
);
CREATE INDEX ide_audit_workspace_at ON ide_audit (workspace, at_ms);
CREATE TRIGGER ide_audit_no_update_delete BEFORE UPDATE OR DELETE ON ide_audit
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER ide_audit_no_truncate BEFORE TRUNCATE ON ide_audit
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
