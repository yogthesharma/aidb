-- Phase 9 demo (run each statement separately via `aidb sql`):
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}');"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_workflow('{\"then\":[{\"search\":{\"query\":\"How do refunds work?\",\"k\":5}},{\"approve\":{\"message\":\"Send this answer?\"}},{\"generate\":{\"prompt\":\"Draft the reply\"}}]}');"
--   cargo run -p aidb-cli -- runs ./app.db --waiting
--   cargo run -p aidb-cli -- sql ./app.db "SELECT id, status, json_extract(output_json, '$.message') FROM runs WHERE status = 'awaiting_approval';"
--   cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_resume('run_…', '{\"approved\":true}');"
-- Parked output_json is JSON {paused, status, message}. The workflow's SQL output
-- column is still the human message.
SELECT aidb_workflow('{"then":[{"search":{"query":"How do refunds work?","k":5}},{"approve":{"message":"Send this answer?"}},{"generate":{"prompt":"Draft the reply"}}]}');
SELECT json_extract(output_json, '$.message') FROM runs WHERE status = 'awaiting_approval';
