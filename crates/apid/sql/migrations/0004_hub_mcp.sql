-- 0004 hub_mcp: the hub as an MCP server (docs/design/hub-mcp.md). hub_mcp_tokens holds one row per token
-- (only its sha256 is kept), hub_mcp_calls is the append-only audit of its calls, and hub_mcp_settings holds
-- the one row of the server's switch, bind and allowed origins.
CREATE TABLE hub_mcp_tokens (
    id text PRIMARY KEY CHECK (id ~ '^mcp_[0-9a-f]{12}$'),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    scope text NOT NULL CHECK (scope IN ('observe', 'operate', 'govern')),
    allowlist text[] NOT NULL DEFAULT '{}',
    token_sha256 bytea NOT NULL UNIQUE CHECK (octet_length(token_sha256) = 32),
    created timestamptz NOT NULL DEFAULT now(),
    expires timestamptz,
    last_used timestamptz,
    calls bigint NOT NULL DEFAULT 0 CHECK (calls >= 0),
    revoked_at timestamptz
);

CREATE TABLE hub_mcp_calls (
    id bigserial PRIMARY KEY,
    token_id text NOT NULL REFERENCES hub_mcp_tokens (id),
    tool text NOT NULL CHECK (char_length(tool) BETWEEN 1 AND 64),
    args_sha256 text NOT NULL CHECK (args_sha256 ~ '^[0-9a-f]{64}$'),
    ok boolean NOT NULL,
    error text CHECK (error IN ('unknown_tool', 'rate_limited', 'tool_error')),
    duration_ms bigint NOT NULL CHECK (duration_ms >= 0),
    at timestamptz NOT NULL
);
CREATE INDEX hub_mcp_calls_token_at ON hub_mcp_calls (token_id, at);
CREATE TRIGGER hub_mcp_calls_no_update_delete BEFORE UPDATE OR DELETE ON hub_mcp_calls
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER hub_mcp_calls_no_truncate BEFORE TRUNCATE ON hub_mcp_calls
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();

CREATE TABLE hub_mcp_settings (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    enabled boolean NOT NULL DEFAULT false,
    bind text NOT NULL DEFAULT 'local' CHECK (bind IN ('local', 'remote')),
    allowed_origins text[] NOT NULL DEFAULT '{}',
    updated timestamptz NOT NULL DEFAULT now()
);
INSERT INTO hub_mcp_settings (singleton) VALUES (true);
