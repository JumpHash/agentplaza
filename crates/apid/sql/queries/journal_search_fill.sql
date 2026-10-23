-- No parameters. Indexes every journal note: its task, its number, its text and its reason (store/search.rs).
INSERT INTO "journal_note_search" ("task", "seq", "text", "reason")
SELECT "journal_task", "seq", COALESCE("text_content", ''), COALESCE("reason_content", '')
FROM "journal_notes"
