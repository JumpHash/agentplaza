-- Whether an earlier retained call of task ?1 ran on a lower rung than ?2 (cheap 0, mid 1, frontier 2).
SELECT EXISTS (
    SELECT 1 FROM "usage_call"
    WHERE "task" = ?1 AND (CASE "rung" WHEN 'cheap' THEN 0 WHEN 'mid' THEN 1 WHEN 'frontier' THEN 2 END) < ?2
)
