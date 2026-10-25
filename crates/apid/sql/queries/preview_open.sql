-- The previews that are not closed, in the table's order: the lifecycle sweep reads them.
SELECT * FROM "preview" WHERE "status" != 'closed' ORDER BY "pos"
