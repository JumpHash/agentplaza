-- Reads the server's settings row: enabled, bind, the allowed origins, the auth mode and the open scope. No parameters.
SELECT enabled, bind, allowed_origins, auth_mode, open_scope FROM hub_mcp_settings WHERE singleton
