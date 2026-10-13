-- Replaces the server's settings row.
-- $1 = enabled (boolean), $2 = bind ('local' or 'remote'), $3 = allowed_origins (text[]),
-- $4 = auth_mode ('secure' or 'open'), $5 = open_scope ('observe', 'operate' or 'govern')
UPDATE hub_mcp_settings SET enabled = $1, bind = $2, allowed_origins = $3, auth_mode = $4, open_scope = $5, updated = now() WHERE singleton
