-- Puts one event (its id is assigned): ?1 t_ms, ?2 kind, ?3 entity kind, ?4 entity, ?5 duration ms (NULL when none), ?6 detail.
INSERT INTO metric_event (t_ms, kind, entity_kind, entity, duration_ms, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6);
