-- Phase 21 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_mcp_register('{\"tools\":[{\"name\":\"send.email\",\"side_effect\":\"irreversible\",\"retry\":\"forbidden\"}]}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_set_policy('{\"read_only\":true,\"deny\":[\"send.email\"],\"max_usd\":0.10}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_get_policy();"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_agent('Email the customer', '[\"send.email\"]');"
-- Policy lives in the file (aidb_meta). Goal CONSTRAINTS and AIDB_MAX_* overlay the same object.
-- Irreversible tools still HITL even if policy says allow. No secrets in the policy.
SELECT aidb_set_policy('{"read_only":true,"deny":["send.email"],"max_usd":0.10}');
