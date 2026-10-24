-- The position a new token takes in the token list: after the last one (0 when there is none). No parameters.
SELECT COALESCE(MAX("pos") + 1, 0) FROM "hub_mcp_token"
