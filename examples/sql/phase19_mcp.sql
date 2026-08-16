-- Phase 19 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-tool --bin fake-mcp
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_mcp_connect('stdio', './target/debug/fake-mcp');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT name, source FROM capabilities WHERE source = 'mcp';"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_tool('echo.ping', '{\"text\":\"hello\"}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_agent('Use the connected MCP tool', '[\"echo.ping\"]');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_mcp_disconnect();"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT name, source FROM capabilities WHERE source = 'mcp';"
-- MCP stdio lists tools into the catalog. Disconnect keeps the rows. Invoke writes kind='tool' runs.
SELECT aidb_mcp_connect('stdio', './fake-mcp');
