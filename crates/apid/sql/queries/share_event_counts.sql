-- The views and the downloads of share ?1, over every event it has. One row: views, then downloads.
-- ?1 = share id
SELECT COUNT(CASE WHEN "kind" = 'view' THEN 1 END), COUNT(CASE WHEN "kind" = 'download' THEN 1 END)
FROM "share_event" WHERE "share" = ?1
