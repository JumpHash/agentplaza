-- Puts one part of a model call's context split: ?1 call id, ?2 position, ?3 part name, ?4 tokens.
INSERT INTO model_call_context (call, pos, part, tokens) VALUES (?1, ?2, ?3, ?4);
