-- No parameters. Indexes every memory entry: its key, its title and its text (store/search.rs).
INSERT INTO "memory_entry_search" ("key", "title", "body")
SELECT "key", "title", "body"
FROM "memory_entry"
