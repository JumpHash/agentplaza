-- Test only: a temp table guarded by forbid_mutation(): one row, a row trigger on UPDATE and DELETE,
-- and a statement trigger on TRUNCATE.
CREATE TEMP TABLE forbid_probe (id int PRIMARY KEY, note text NOT NULL);
INSERT INTO forbid_probe VALUES (1, 'kept');
CREATE TRIGGER forbid_probe_rows BEFORE UPDATE OR DELETE ON forbid_probe FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER forbid_probe_truncate BEFORE TRUNCATE ON forbid_probe FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
