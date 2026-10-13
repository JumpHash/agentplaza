-- 0005 hub_mcp modes: the owner chooses secure (a token is required, the default) or open (a call with no token gets
-- the open scope the owner picks, observe by default). The audit log records an anonymous call with no token and the
-- caller's IP. Applied migrations are immutable, so 0004 is not edited.
ALTER TABLE hub_mcp_settings
    ADD COLUMN auth_mode text NOT NULL DEFAULT 'secure' CHECK (auth_mode IN ('secure', 'open')),
    ADD COLUMN open_scope text NOT NULL DEFAULT 'observe' CHECK (open_scope IN ('observe', 'operate', 'govern'));

ALTER TABLE hub_mcp_calls
    ALTER COLUMN token_id DROP NOT NULL,
    ADD COLUMN client_ip text CHECK (char_length(client_ip) BETWEEN 1 AND 64),
    ADD CONSTRAINT hub_mcp_calls_caller CHECK (token_id IS NOT NULL OR client_ip IS NOT NULL);
