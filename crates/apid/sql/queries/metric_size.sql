-- The bytes one table and its indexes take on disk: ?1 table name.
SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name IN (SELECT name FROM sqlite_schema WHERE tbl_name = ?1);
