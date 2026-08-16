# SQL surface

SQL is the product. Bindings wrap these functions; they do not grow a second
engine. Run each statement with `aidb sql ./app.db "…"` or `db.query("…")`.

## Documents and search

```sql
SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{"dept":"support"}');
SELECT document_id, content, distance FROM aidb_search('how do refunds work?', 3);
SELECT document_id FROM aidb_search('refund policy', 5, '{"dept":"support"}');
```

Hybrid search (FTS + vec) is the same function. `EXPLAIN SELECT aidb_search(…)`
prints the plan. Named embedding spaces:

```sql
SELECT aidb_create_space('legal', 'fake', 8, 'aidb-fake', 'cosine');
SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal');
```

Memory is documents with `metadata.kind = 'memory'`:

```sql
SELECT aidb_memory_insert('user:123', 'Prefers concise answers.');
SELECT * FROM aidb_memory_search('user:123', 'How should I explain this?', 5);
```

## Generate, classify, schema

Two-arg generate returns text. Over `aidb_search` it returns `{ answer, sources[] }`.
A third argument is a JSON Schema; invalid model output fails the run.

```sql
SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.');
SELECT aidb_generate(
  'Extract a summary',
  'Refunds take 14 days.',
  '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'
);
SELECT aidb_classify('positive or negative', 'This refund was a negative surprise.');
SELECT aidb_last_run_id();  -- this connection's last insert; empty in a new process
```

## Runs, tokens, sessions

Every generate / search / tool / agent / workflow is a row in `runs`. Tokens
append to the generate run as `run_events` (`kind = 'token'`).

```sql
SELECT id, kind, status, cost_usd, error FROM runs ORDER BY created_at_ms DESC LIMIT 10;
SELECT seq, json_extract(payload_json, '$.text') AS text
  FROM run_events
 WHERE kind = 'token'
   AND run_id = (SELECT id FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1)
 ORDER BY seq;

SELECT aidb_session('desk:nvda');
SELECT turn, kind FROM session_turns WHERE session_id = 'desk:nvda' ORDER BY turn;
```

`sessions` and `session_turns` are views. There is no sessions table.

## Agents, workflows, HITL

```sql
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":4}');
SELECT aidb_agent('{"decide":true,"instructions":"Answer from documents. End with DONE.","goal":"Brief me on NVDA only","tools":["search","generate"],"max_steps":4}');
SELECT aidb_workflow('{"then":[{"search":{"query":"refunds","k":3}},{"approve":{"message":"send?"}},{"generate":{"prompt":"Draft"}}]}');
SELECT id, json_extract(output_json, '$.message') FROM runs WHERE status = 'awaiting_approval';
SELECT aidb_resume('run_…', '{"approved":true}');
```

Parked `output_json` is `{"paused":true,"status":…,"message":…}`. The SQL `output`
column from the agent/workflow call stays the human message. Irreversible tools
park; they do not run until `aidb_resume`.

## Policy, tools, experiments

```sql
SELECT aidb_set_policy('{"allow":["search","generate"],"max_usd":1}');
SELECT aidb_tool('send.email', '{"to":"alice@desk.test"}');
SELECT aidb_experiment('{"dataset":"support_gold","plans":["naive","cascade"],"k":3}');
SELECT plan, accuracy, cost_usd FROM experiment_results ORDER BY cost_usd;
```

Demo scripts per phase live in [`examples/sql/`](../examples/sql/).
