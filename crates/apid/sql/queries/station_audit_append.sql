-- Appends one owner action or lease change to the station audit (stations/ builds the row; seq is generated).
-- $1 = at_ms (bigint), $2 = station (text, null for the registry as a whole), $3 = agent (bigint, null when none),
-- $4 = kind (text), $5 = the whole row (jsonb)
INSERT INTO station_audit (at_ms, station, agent, kind, data) VALUES ($1,$2,$3,$4,$5)
