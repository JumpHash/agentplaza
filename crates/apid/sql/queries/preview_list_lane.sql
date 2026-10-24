-- The previews of lane ?2 of project ?1, newest first (the newest one is the lane's current preview).
-- ?1 = project, ?2 = lane
SELECT * FROM "preview" WHERE "project" = ?1 AND "lane" = ?2 ORDER BY "pos" DESC
