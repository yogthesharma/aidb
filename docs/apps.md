# Projects you can build on AIDB

AIDB is the file, not the product UI. Each row is an AI application whose
durable state is SQLite: documents, embeddings, runs, tools, approvals, spend.

The application owns ingest, domain tables, and screens. Copy `app.db` and you
copied the audit trail. There is no vector service or trace backend to keep in
sync.

Shipped examples: [`examples/stock`](../examples/stock/README.md) (equity research
desk, CLI), [`examples/support`](../examples/support/README.md) (support desk),
and [`examples/chat`](../examples/chat/README.md) (ChatGPT-style chat on an empty
file). SQL for the primitives: [`sql.md`](sql.md).

| Project | What it does | In the file | You write |
| --- | --- | --- | --- |
| Cited knowledge assistant | Answer only from a corpus, with sources | `aidb_search`, generate-over-search `{answer, sources[]}` | ingest, the prompt, the UI |
| Support / refunds bot | Keyword + semantic retrieval on policy docs | hybrid `aidb_search`, metadata filter | department tags — **this repo** (`examples/support`) |
| Equity / research desk | Filings → brief → classify headlines → email digest | search, generate, classify, agent, HITL | `watchlist` / `signals` — **this repo** |
| Ticket or headline classifier | Label a row, link it to the generate run | `aidb_classify`, `aidb_last_run_id()` | `INSERT INTO tickets …` |
| Structured extraction | Schema-valid JSON from a filing, invoice, or email | `aidb_generate(…, schema)`; mismatch fails the run | the schema, destination columns |
| Approval-gated ops agent | Draft, then park before `send.email` / writes | agent or workflow, `awaiting_approval`, `aidb_resume` | the irreversible tool |
| Decide-loop analyst | Model picks search vs generate vs tool, with args | `"decide":true`, filters, tool JSON args | the goal, `max_steps` |
| Personal memory assistant | “Remember that I prefer short answers” | `aidb_memory_insert` / `_search` (documents) | user id; not a messages table |
| Threaded research session | Turn 1 / 2 / 3 over the same desk | `aidb_session`, `session_turns` view | a chat UI — **this repo** (`examples/chat`) |
| Eval / plan bakeoff | Naive vs cascade under a budget | `aidb_experiment`, `experiment_results` | labeled gold questions |
| Multi-space search | Legal embeddings ≠ product embeddings | `aidb_create_space`, `aidb_search(..., space)` | which corpus uses which space |
| Policy-bounded copilot | Deny-list, USD/ms cap, HITL overlay | `aidb_set_policy`, capability catalog | the policy JSON |
| Incident / RCA helper | Task + constraints → workflow run | goal language → IR → workflow | logs/deploy tables, the TASK text |
| Live generate inspect | Reconnect still has the token prefix | `run_events` `kind='token'`, `GET /runs/{id}/events` | Studio or your WS client |
| MCP-backed tools | Local stdio tools in the same catalog | `aidb_mcp_connect`, `kind='tool'` runs | the MCP server binary |

## What not to build as an AIDB primitive

These belong to the application or a provider, not a new table in the engine:

- ChatGPT clone / conversations table (use `session_turns` or your own UI)
- An `agents` table (an agent is a parent run)
- A second store, vector SaaS, or LangSmith-style trace backend
- Hosting models (keys stay in the environment)
- DataFusion until a profile says SQLite is the bottleneck

## Pattern

```text
your tables  ─┐
documents    ─┤
runs         ─┼─  one file  ─  AIDB
approvals    ─┤
spend        ─┘
```

Start from a `SELECT`. If the app needed a primitive that belongs in the file,
that is a [phase](../PHASES.md) discussion — not a graph library.
