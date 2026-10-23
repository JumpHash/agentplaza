-- ?1 the match expression (words, each quoted), ?2 the task. The task's notes that match, best match first.
SELECT "seq"
FROM "journal_note_search"
WHERE "journal_note_search" MATCH ?1 AND "task" = ?2
ORDER BY bm25("journal_note_search"), "seq"
