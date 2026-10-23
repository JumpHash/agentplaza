-- The previews of project ?1, newest first.
-- ?1 = project
SELECT * FROM "preview" WHERE "project" = ?1 ORDER BY "pos" DESC
