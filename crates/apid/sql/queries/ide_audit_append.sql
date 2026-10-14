-- Appends one owner action to the IDE audit (ide.rs builds the row; seq is generated).
-- $1 = at_ms (bigint), $2 = workspace (text), $3 = kind (text), $4 = the whole row (jsonb)
INSERT INTO ide_audit (at_ms, workspace, kind, data) VALUES ($1,$2,$3,$4)
