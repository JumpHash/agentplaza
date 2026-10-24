-- Marks artifact ?1 shared (?2 = 1) or not (?2 = 0).
-- ?1 = artifact id, ?2 = shared (0 or 1)
UPDATE "artifact" SET "shared" = ?2 WHERE "id" = ?1
