-- 0009 station_audit: the owner's actions on workstations and every lease change (crates/apid/src/stations/):
-- join codes, joins, revokes, drains, settings, STOP ALL and the leases. One row per action; data is the whole row
-- the audit returns (at_ms, station, agent, kind, actor and the action's own fields), and the columns repeat the
-- ones the audit filters and orders by. station is null for an action on the registry as a whole (a join code).
-- Append-only, as ide_audit is. The registry itself is the `stations` component in plaza_state, as nodes are.
CREATE TABLE station_audit (
    seq bigserial PRIMARY KEY,
    at_ms bigint NOT NULL,
    station text,
    agent bigint,
    kind text NOT NULL,
    data jsonb NOT NULL
);
CREATE INDEX station_audit_station_at ON station_audit (station, at_ms);
CREATE TRIGGER station_audit_no_update_delete BEFORE UPDATE OR DELETE ON station_audit
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER station_audit_no_truncate BEFORE TRUNCATE ON station_audit
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
