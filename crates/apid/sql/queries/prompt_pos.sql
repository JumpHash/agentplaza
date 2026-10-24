-- The place of prompt ?1 in the prompt table: the one it has, else the next after the highest (0 for the first).
-- ?1 = prompt id
SELECT COALESCE((SELECT "pos" FROM "prompt" WHERE "id" = ?1), (SELECT COALESCE(MAX("pos"), -1) + 1 FROM "prompt"))
