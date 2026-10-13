-- Appends one history row. $1 = skill id, $2 = action, $3 = version (or null), $4 = actor, $5 = now_ms (bigint)
INSERT INTO skill_events (skill_id, action, version, actor, "at")
VALUES ($1, $2, $3, $4, to_timestamp(($5::bigint / 1000.0)::double precision))
