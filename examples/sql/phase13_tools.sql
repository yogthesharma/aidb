-- Phase 13 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT name, side_effect FROM capabilities;"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_mcp_register('{\"tools\":[{\"name\":\"github.read\",\"inputs\":{\"path\":\"string\"},\"side_effect\":\"none\",\"retry\":\"safe\"}]}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_agent('{\"instructions\":\"Answer from documents. End with DONE.\",\"goal\":\"How do refunds work?\",\"tools\":[\"search\",\"github.read\"],\"max_steps\":2}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT kind, status, input_json FROM runs WHERE kind IN ('agent','tool','search') ORDER BY created_at_ms;"
-- Irreversible tools park for approval (Phase 9). They do not send email or POST.
SELECT name, side_effect FROM capabilities;
