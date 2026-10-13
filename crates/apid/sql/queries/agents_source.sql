-- The agents data migration source: the plaza_state blob as text, and its sha256 (hex). No row when
-- plaza_state holds no agents snapshot. No parameters.
SELECT data::text, encode(sha256(convert_to(data::text, 'UTF8')), 'hex') FROM plaza_state WHERE component = 'agents'
