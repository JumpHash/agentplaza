-- A replay ended: ?1 its id, ?2 its outcome, ?3 whether it matched the known outcome, ?4 the cost its task spent, ?5 the
-- time it took.
UPDATE "eval_bench" SET "outcome" = ?2, "matched" = ?3, "cost_micros" = ?4, "wall_ms" = ?5 WHERE "id" = ?1
