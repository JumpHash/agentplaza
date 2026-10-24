-- The newest audit seq of the hub MCP calls (0 when there is none); the next call takes this + 1. No parameters.
SELECT COALESCE(MAX("seq"), 0) FROM "hub_mcp_call"
