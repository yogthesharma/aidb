-- Phase 32 — structured generate
--
--   aidb sql app.db "<each statement below>"
--
-- Two-arg generate is still free text. A third argument is a JSON Schema: the
-- model has to return JSON that matches it. Invalid output fails the run — a
-- row you SELECT — so the app does not parse exception types. Junk schema JSON
-- is a usage error and does not open a run at all.

-- 1. Untyped generate is unchanged.
SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.');

-- 2. The same call with a schema returns canonical JSON, not prose.
SELECT aidb_generate(
  'Extract a summary',
  'Refunds are issued within 14 days of purchase.',
  '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'
);

-- 3. That attempt is a generate run. The schema is on input_json; the JSON is
--    on output_json. Status is succeeded when the model matched.
SELECT status,
       json_extract(input_json, '$.schema.required[0]') AS required,
       json_extract(output_json, '$.text') AS text
  FROM runs
 WHERE kind = 'generate'
 ORDER BY created_at_ms DESC
 LIMIT 1;

-- 4. Classify can take an enum schema. The result is a JSON string in the set.
SELECT aidb_classify(
  'positive or negative',
  'This refund was a negative surprise.',
  '{"enum":["positive","negative"]}'
);

-- 5. FROM documents and FROM aidb_search take the same third argument.
SELECT aidb_generate(
  'Extract a summary',
  content,
  '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'
) FROM documents;

SELECT aidb_generate(
  'Extract a summary',
  content,
  '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'
) FROM aidb_search('refunds', 3);

-- A mismatch (required field the model cannot fill) errors the statement and
-- leaves that generate run as failed, with the raw text and schema_error in
-- output_json. Spend is still on the row. That statement is not in this demo
-- because the examples suite requires every pasted statement to succeed.
