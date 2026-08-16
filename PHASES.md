# AIDB phases and repository layout

This is the build plan. Product and architecture stay in [`DESIGN.md`](DESIGN.md). This file decides **order**, **SQL-first surface**, and **folder structure**.

**Phases 0–8 are the v0 ship** (Rust + SQL engine, then thin TS/Python faces, then agents as runs). Phases 9–26 are done, Phase 28 turned them into an executable specification, Phase 29 proved them with an application that uses AIDB as its only AI runtime, Phase 31 made the optimizer’s cost claim a queryable result, Phase 30 is the inspect face over that file, Phase 32 made generate/classify take a JSON schema so invalid output fails the run, Phase 33 made the agent a decide loop: the model chooses the next operator and its arguments, Phase 34 made a session a thread of runs — Turn 1 / 2 / 3 is a view, not a second store — and Phase 35 made generate tokens durable run events so a reconnect still has the prefix. Phase 27 is DataFusion — last, only if a profile says SQLite is the bottleneck. Do not start remaining work by rewriting `schema/v001.sql` or adding a second store.

v0 surface:

- **Rust + SQL** is still the engine. Open a file, run SQL, get rows.
- **Bindings are faces.** `AI.open` / Python wrap the same `aidb` / `aidb-ffi`. No second run table.

```text
Rust
  Aidb::open("./app.db")
  db.execute("...")
  db.query("...")
        │
        ▼
   SQLite + our functions
        │
        ▼
      app.db
```

Bindings exist as faces (Phase 7). They wrap the same file and the same SQL. They are not a second engine.

```ts
import { AI } from "aidb";
const db = await AI.open("./app.db");
```

---

## 1. How to read the phases

Each phase has a goal, what is in, what is out, the SQL (or Rust) you can run when it is done, and which crates you touch.

Rules:

- A phase is done when the SQL / CLI demo works, not when the crate list looks complete.
- Do not start the next phase’s *product* work early. You may add a crate stub so the folder exists.
- Later phases must not force a rewrite of `schema/` or `crates/aidb-storage`.
- Remaining *product* work is Phase 27, last, only if a profile says SQLite is the bottleneck.
- Phase 28 (tests) is done and runs before Phase 27: a behaviour change has to keep the suite green or fix a contract on purpose.
- After Phase 28, a new primitive is allowed only if a real AIDB-only app needed it and it belongs in the file (not the application, not a provider).

---

## 2. Repository layout

This layout is the one we grow into. Early phases only *implement* a few crates. The rest exist so new work has a home.

```text
aidb/
├── Cargo.toml                 workspace
├── DESIGN.md
├── PHASES.md
├── schema/
│   └── v001.sql … v009.sql    canonical file format (SCHEMA_VERSION 9)
├── crates/
│   ├── aidb/                  public Rust crate: open, execute, query
│   ├── aidb-core/             Error, ids, status enums, meta keys
│   ├── aidb-storage/          SQLite open, pragmas, migrate, writer / readers
│   ├── aidb-sql/              custom SQL functions and table-valued functions
│   ├── aidb-index/            documents, chunk, embed, vec, FTS
│   ├── aidb-run/              runs, events, checkpoints, resume
│   ├── aidb-ai/               model / embedding adapters (space owns the function)
│   ├── aidb-ir/               logical / physical plan (from Phase 4)
│   ├── aidb-opt/              rewrites (Phase 6; scale in Phase 11)
│   ├── aidb-cli/              `aidb` binary: open a file, run SQL
│   ├── aidb-ffi/              C ABI (Phase 7)
│   ├── aidb-node/             napi addon (Phase 12)
│   ├── aidb-python/           PyO3 module (Phase 12)
│   ├── aidb-tool/             capability catalog + tool runtime (Phase 13)
│   └── aidb-server/           HTTP face over the same file (Phase 23)
├── bindings/
│   ├── typescript/            `AI.open` (napi, in-process)
│   └── python/                `AI.open` (PyO3, in-process)
├── studio/                    inspect face over `aidb serve` (Phase 30)
├── tests/
│   └── sql/                   `*.sql` fixtures and expected results
└── examples/
    └── sql/                   demo scripts per phase
```

### Crate roles

| Crate | Depends on (eventually) | Job |
| --- | --- | --- |
| `aidb` | storage, sql, index, run, ai, ir, opt | The crate an app depends on |
| `aidb-core` | — | Shared types. No SQLite. No HTTP. |
| `aidb-storage` | core | One writer, read pool, WAL, apply `schema/v001.sql` … `v009.sql` |
| `aidb-sql` | core, storage, index, run, ai | Register `aidb_search`, `aidb_generate`, classify, session, `aidb_last_run_id` |
| `aidb-index` | core, storage, ai, run | Document write path and async index |
| `aidb-run` | core, storage | `runs` / `run_events` / `checkpoints` |
| `aidb-ai` | core | Embed / chat adapters (`fake` / `openai` / `local` / `custom`). Keys from env, then optional store |
| `aidb-ir` | core | Logical / physical plan (Phase 4) |
| `aidb-opt` | ir | Rewrites (Phase 6; scale in Phase 11) |
| `aidb-cli` | aidb, aidb-server | `aidb sql` / `aidb runs` / optional `aidb serve` |
| `aidb-ffi` | aidb | C ABI. Optional for other languages |
| `aidb-node` | aidb | napi addon. TypeScript loads this |
| `aidb-python` | aidb | PyO3 module. Python loads this |
| `aidb-tool` | core, storage, run | Capability catalog, policy, MCP register + live stdio client |
| `aidb-server` | aidb | HTTP in front of the same file (Phase 23). Not a second engine |

Dependency direction (never invert):

```text
aidb-core
    ↑
aidb-storage
    ↑
aidb-run    aidb-ai
    ↑           ↑
    └── aidb-index
            ↑
        aidb-sql
        aidb-tool
            ↑
          aidb
            ↑
        aidb-cli
          aidb-ffi
          aidb-node
          aidb-python
          aidb-server
            ↑
     bindings/typescript
     bindings/python

aidb-ir  →  aidb-opt     (wired into aidb from Phase 4 / 6)
```

`bindings/typescript` and `bindings/python` wrap `aidb` via napi / PyO3. They do not get their own storage layer. They do not spawn `aidb sql`.

---

## 3. Public surface (v0)

```rust
let db = aidb::open("./app.db")?;
db.execute("INSERT INTO documents (id, title, content, content_hash, created_at_ms, updated_at_ms)
            VALUES (?, ?, ?, ?, ?, ?)", params)?;
let rows = db.query("SELECT * FROM aidb_search(?, 5)", [query])?;
```

CLI:

```bash
aidb sql ./app.db "SELECT id, title, index_status FROM documents;"
aidb sql ./app.db "SELECT * FROM aidb_search('How do refunds work?', 5);"
```

v0 SQL is **ordinary SQLite** plus a small set of functions we register. `SEARCH` / `CREATE MODEL` / `AI_GENERATE` are Phase 15 convenience: they lower to those functions and the same IR.

---

## Phase 0 — Workspace and open

**Status: done.**

**Goal:** `aidb::open("./app.db")` creates or opens a file, applies pragmas, applies `schema/v001.sql`.

**In**

- Cargo workspace and the crate folders above
- `aidb-core` errors / ids
- `aidb-storage` WAL, foreign keys, busy timeout, migrate v001
- `aidb` `open` / `execute` / `query` over ordinary SQL
- `aidb-cli` `aidb sql <file> <sql>`
- `vec_chunks` is **not** created yet (dimensions unknown)

**Out**

- Documents helpers, embeddings, search function, models, IR, CLI REPL polish

**Done when**

```bash
aidb sql ./app.db "SELECT value FROM aidb_meta WHERE key = 'schema_version';"
# 1
```

A second `open` is a no-op migrate. `PRAGMA journal_mode` is WAL.

**Crates:** `aidb-core`, `aidb-storage`, `aidb`, `aidb-cli`

---

## Phase 1 — Documents, async index, search

**Status: done.** Hybrid search is Phase 10.

**Goal:** The first product demo, over SQL.

```text
open → INSERT document → async index → aidb_search → rows
```

**In**

- `INSERT INTO documents` (or a small helper that fills hash / timestamps / `pending`)
- Same transaction: `runs` row `kind = 'index_document'`
- Background index: chunk → embed → `vec_chunks` upsert → `index_status = ready | failed`
- Create `vec_chunks` on first embed, once dimensions are known (`aidb_meta`)
- FTS triggers already in v001 — keep them working
- Table-valued function `aidb_search(query TEXT, k INTEGER)`
  - embeds the query
  - KNN on `vec_chunks`
  - joins `chunks` + `documents`
  - only `index_status = 'ready'`
- One embedding space per file; mismatch on open fails closed
- Keys from the environment; `aidb-ai` can start with a fake embedder for tests plus one real provider

**Out**

- Hybrid search, RAG citations, `AI_GENERATE`, workflow, optimizer, agents, TS
- Custom `SEARCH` keyword in the parser

**SQL when done**

```sql
INSERT INTO documents (id, title, content, metadata_json, content_hash, created_at_ms, updated_at_ms)
VALUES ('01h…', 'Refunds', 'Refunds are issued within 14 days…', '{}', '…', 0, 0);

SELECT id, index_status FROM documents;
-- pending → indexing → ready

SELECT document_id, chunk_id, content, distance
FROM aidb_search('How do refunds work?', 5);
```

**Done when** the CLI can insert a document, wait until `ready`, and `aidb_search` returns the chunk. Crash during embed: restart, resume missing vec rows (first `kill -9` if easy; otherwise Phase 3).

**Crates:** `aidb-index`, `aidb-ai`, `aidb-run` (minimal), `aidb-sql`

---

## Phase 2 — Models and generation

**Status: done.** `CREATE MODEL` dialect is Phase 15.

**Goal:** `AI_GENERATE` as a SQL function that still goes through the run engine.

**In**

- `models` catalog (`INSERT` / `SELECT`, no keys in the file)
- `ai_generate(prompt, content)` or `aidb_generate(...)` → text
- Each call inserts `runs.kind = 'generate'`
- Env-only provider keys
- Optional: structured output later in this phase, not required to close it

**Out**

- SQL dialect `CREATE MODEL`
- Model hosting, routing, vision

**SQL when done**

```sql
INSERT INTO models (name, kind, provider, provider_model, created_at_ms)
VALUES ('gpt', 'llm', 'openai', 'gpt-4.1-mini', 0);

SELECT aidb_generate('Summarize this', content) FROM documents WHERE id = '01h…';

SELECT id, status, prompt_tokens, cost_usd FROM runs WHERE kind = 'generate';
```

**Crates:** `aidb-ai`, `aidb-sql`, `aidb-run`

---

## Phase 3 — Runs as a first-class SQL object

**Status: done.** Approval / wait states are Phase 9.

**Goal:** Execution is queryable data. Resume is real for `index_document` and `generate`.

**In**

- Stable `runs` / `run_events` / `checkpoints` usage
- `aidb_search` / generate / index all write runs
- Resume after `kill -9` for index (chunk done, embed not)
- `SELECT` observability: failed runs, slow runs, cost

**Out**

- Workflow kinds beyond linear operators
- Approval / wait states

**SQL when done**

```sql
SELECT * FROM runs WHERE status = 'failed';
SELECT * FROM run_events WHERE run_id = ? ORDER BY seq;
```

**Crates:** `aidb-run`, `aidb-index`, `aidb-cli` (optional `aidb runs` list)

---

## Phase 4 — IR

**Status: done.**

**Goal:** SQL and Rust helpers lower to a logical plan. Still no clever optimizer.

**In**

- `aidb-ir`: operators, schemas, contracts
- Binder: tables exist, models exist, functions exist
- Physical bind: SQLite vs AI runtime
- `EXPLAIN` or `aidb_explain('…')` prints the plan
- Internal only is fine; no new user DSL required

**Out**

- Cost-based choice, cascade TopK, model selection

**Done when** `aidb_search` and `aidb_generate` go through IR even if the plan is trivial.

**Crates:** `aidb-ir`, wire into `aidb` / `aidb-sql`

---

## Phase 5 — Workflow compile

**Status: done.** HITL is Phase 9. MCP is Phases 13 / 19.

**Goal:** `then` / `parallel` / `branch` / `loop` exist as IR (and maybe SQL/JSON), not as a LangGraph SDK.

**In**

- Workflow as data: a declared graph that compiles to IR
- Persist as `runs.kind = 'workflow'` + child runs + checkpoints
- Checkpoint after each operator
- SQL to submit / inspect, not a TypeScript `workflow.parallel()`

**Out**

- Agents, HITL, MCP, goal-from-English planner

**Crates:** `aidb-ir`, `aidb-run`, thin API on `aidb`

---

## Phase 6 — Optimizer

**Status: done (small labeled set).** 10k-row / hard $ and ms budgets are Phase 11.

**Goal:** The engine chooses Plan B.

**In**

- Three rewrite classes: equivalence, approximation, physical
- `PushFilterBeforeExpensive`
- `CascadeEmbedTopKThenJudge` with sample-vs-gold
- Batch / keyed cache
- Budgets: max USD, max latency
- Printed plan + measured $ / ms / tokens

**Out**

- Fake “95% expected quality”
- Goal language
- Agents

**Done when** a small labeled workload is cheaper than naive per-row LLM, quality held on a sample, plan is readable. Scale and hard USD/ms enforcement are Phase 11.

**Crates:** `aidb-opt`

---

## Phase 7 — Bindings

**Status: done (thin faces).** Native napi / PyO3 is Phase 12 (done).

**Goal:** `AI.open` / Python open the same file as `aidb sql`.

**In**

- `aidb-ffi` C ABI: open, sql, drain, close
- TypeScript `AI.open` (`documents.insert`, `search`, `query`, `agent.run`)
- Python `AI.open` (ctypes in this phase; native PyO3 is Phase 12)
- No second storage engine. No second run table

**Out**

- napi-rs / PyO3 (Phase 12)
- A TypeScript-only runtime

**Done when** `python3 bindings/python/test_open.py` and `node bindings/typescript/test.mjs` open a file, insert, search.

**Crates / folders:** `aidb-ffi`, `bindings/typescript`, `bindings/python`

---

## Phase 8 — Agents

**Status: done (v0).** HITL is Phase 9. Memory is Phase 14. MCP is Phases 13 / 19.

**Goal:** Agent = model + instructions + tools + memory + loop, persisted as child runs.

**In**

- `SELECT aidb_agent('{…}')` → parent `runs.kind = 'agent'`
- Tools in v0: `search`, `generate`
- Checkpoint after each loop step
- No `agents` table in v001

**Out**

- HITL, MCP, shared memory tables, multi-agent (Phases 9, 13, 14)

**SQL when done**

```sql
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":3}');
SELECT id, parent_id, kind, status FROM runs;
```

**Crates:** `aidb`, `aidb-sql`, `aidb-run`

---

## Phase 9 — Human-in-the-loop

**Status: done.**

**Goal:** Approval and wait are run states, not IR nodes.

**In**

- Run statuses: `suspended`, `awaiting_approval` (extend CHECK via migrate, do not add workflow tables)
- `aidb_resume(run_id, '{"approved":true}')` or `db.runs.resume(id, { approved: true })`
- Parked `output_json` is JSON `{"paused":true,"status":…,"message":…}`; the SQL `output` column stays the human message
- Checkpoint already exists; resume continues the next operator
- SQL to list waiting runs

**Out**

- Approval as an IR operator
- Policy language, encrypted secret stores

**SQL when done**

```sql
SELECT aidb_workflow('{"then":[{"search":{"query":"How do refunds work?","k":5}},{"approve":{"message":"Send this answer?"}},{"generate":{"prompt":"Draft the reply"}}]}');
SELECT id, status, json_extract(output_json, '$.message') FROM runs WHERE status = 'awaiting_approval';
SELECT aidb_resume('run_…', '{"approved":true}');
```

Parked `output_json` is always JSON `{"paused":true,"status":…,"message":…}`. The SQL `output` column from `aidb_workflow` / `aidb_agent` stays the human message.

**Crates:** `aidb-run` (`park_run` JSON), `aidb-sql`, `aidb-cli`, `aidb-tool`

---

## Phase 10 — Hybrid search

**Status: done.**

**Goal:** FTS + vec as a physical plan, not a new user API.

**In**

- Physical rewrite: `aidb_search` can run vec KNN, FTS, or a blend
- `EXPLAIN` shows which algorithm ran
- Still one function: `aidb_search(query, k)`
- Citations / chunk provenance optional if cheap

**Out**

- A second `SEARCH` product
- Replacing sqlite-vec

**Done when** a keyword-heavy query that misses on vec-only hits via FTS (or the blend), and the plan is readable.

**SQL when done**

```sql
EXPLAIN SELECT document_id, chunk_id, content, distance
FROM aidb_search('How do refunds work ZX19QPLUGH', 3);

SELECT document_id, chunk_id, content, distance
FROM aidb_search('How do refunds work ZX19QPLUGH', 3);
```

**Crates:** `aidb-index`, `aidb-opt`, `aidb-sql`

---

## Phase 11 — Optimizer at scale

**Status: done.** Gold job is a labeled 256-row set (10k is the same plan). `AIDB_MAX_USD` / `AIDB_MAX_MS` are enforced on every provider, including fake.

**Goal:** Phase 6 on a real labeled workload, with hard $ and latency budgets.

**In**

- 10k-row (or published smaller gold) job cheaper than naive per-row LLM
- Enforce `AIDB_MAX_USD` and `AIDB_MAX_MS` on a real provider, not only `AIDB_MAX_LLM_CALLS`
- Widen `k` or fall back when sample recall misses the floor
- Printed plan + measured $ / ms / tokens on the run

**Out**

- Fake “95% expected quality”
- Goal language (Phase 16)

**SQL when done**

```sql
EXPLAIN SELECT aidb_generate('How do refunds work?', content) FROM documents;
SELECT aidb_generate('How do refunds work?', content) FROM documents;
SELECT prompt_tokens, completion_tokens, cost_usd, output_json
FROM runs WHERE kind = 'generate';
```

**Crates:** `aidb-opt`, `aidb-ai`, `aidb-run`

---

## Phase 12 — Native bindings

**Status: done.** Same `AI.open` API. TypeScript loads napi; Python loads PyO3. No CLI spawn, no ctypes.

**Goal:** Same `AI.open` API, loaded in-process.

**In**

- TypeScript via napi (or equivalent) on `aidb` / `aidb-ffi`
- Python via PyO3 (or equivalent)
- Embedding options on open (provider / model / dimensions). Keys still from the environment
- Drop the CLI subprocess path as the default TS face

**Out**

- A second engine inside Node or CPython
- Rewriting the public `AI.open` shape

**Done when** `node bindings/typescript/test.mjs` and `python3 bindings/python/test_open.py` open a file, insert, search, and run an agent without spawning `aidb sql` or using ctypes.

```text
cargo build -p aidb-node -p aidb-python
node bindings/typescript/test.mjs
python3 bindings/python/test_open.py
```

**Folders:** `crates/aidb-node`, `crates/aidb-python`, `bindings/typescript`, `bindings/python`

---

## Phase 13 — Tools and MCP

**Status: done.** Capabilities are catalog rows. MCP registers into that table. Irreversible tools park for approval.

**Goal:** Capabilities are catalog data. MCP is an adapter, not a product.

**In**

- Capability rows (or `models`-like catalog): name, inputs, outputs, side effect, retry
- Tool calls persist as child runs (`kind` already in the run engine, or `input_json` on generate/tool)
- Allow-list / deny-list (e.g. no `send.email` unless approved — ties to Phase 9)
- Optional MCP client that *registers* capabilities. The optimizer/runtime sees the same catalog

**Out**

- MCP as the user-facing API
- Tool POST / email without policy
- Agents owning their own tool runtime

**SQL when done**

```sql
SELECT name, side_effect FROM capabilities;
SELECT aidb_mcp_register('{"tools":[{"name":"github.read","side_effect":"none"}]}');
SELECT aidb_agent('{"instructions":"…","goal":"…","tools":["search","github.read"]}');
SELECT kind, status, input_json FROM runs WHERE parent_id = ?;
```

**Crates:** `aidb-tool`, `aidb-run`, `aidb-sql`

---

## Phase 14 — Memory and multi-agent

**Status: done.** Memory is a view over documents. Multi-agent is a parent `agent` run with child `agent` runs. No `agents` table.

**Goal:** Shared memory is tables. Multi-agent is composition of runs.

**In**

- Memory as documents (or a small `memory` view over documents), searchable with `aidb_search`
- `db.memory.insert` / SQL helper that writes a document + index run
- Multi-agent: parent agent run with child `kind = 'agent'` runs
- Still no permanent `agents` table

**Out**

- Hidden context objects
- A second graph store

**SQL when done**

```sql
SELECT aidb_memory_insert('user:123', 'Prefers concise technical explanations.');
SELECT document_id, content FROM aidb_search('How should I explain this?', 5);
```

**Crates:** `aidb-index`, `aidb`, bindings

---

## Phase 15 — SQL dialect

**Status: done.** `SEARCH` / `CREATE MODEL` / `AI_GENERATE` lower to the same IR and catalog as the functions. No second planner.

**Goal:** `SEARCH` / `CREATE MODEL` / `AI_GENERATE` as syntax that lowers to the same IR.

**In**

- Parser convenience only. Same functions, same runs, same optimizer
- `CREATE MODEL` writes `models` (still no keys in the file)
- `SEARCH '…' LIMIT 5` ≡ `aidb_search('…', 5)`

**Out**

- A second planner
- Inventing dialect before the functions stay correct (they already are)

**SQL when done**

```sql
CREATE MODEL gpt (kind = llm, provider = openai, provider_model = 'gpt-4.1-mini');

SELECT * FROM documents
SEARCH 'How do refunds work?'
LIMIT 5;
```

**Crates:** `aidb-sql`, `aidb-ir`

---

## Phase 16 — Goal language

**Status: done.** `TASK` / `DATA` / `CONSTRAINTS` / `GOAL` compile to IR. The optimizer rewrites that plan. Execution is a workflow run, not a goals table.

**Goal:** A frontend that emits IR. Only after Phase 11.

```text
TASK investigate_incident
DATA logs, deployments
CONSTRAINTS read_only, budget $1, timeout 5m
GOAL identify_root_cause
```

**In**

- Compile goal + data + constraints → logical IR
- Optimizer (Phase 11) must be able to rewrite that IR
- Persist as a run (`workflow` or `agent`), not a new goal store

**Out**

- NL2SQL as the product
- Skipping the optimizer and emitting a hand-written DAG

**Crates:** `aidb-ir`, `aidb-sql`, `aidb-opt`

---

## Remaining work (audit)

This table was the DESIGN leftover list after Phase 16. Those items shipped as Phases 17–26. Phases 28–35 (and `aidb_last_run_id` / parked JSON) followed from the suite and the stock desk. Remaining numbered product work is only Phase 27.

| Item | Shipped as |
| --- | --- |
| AI providers / classify | Phase 20 |
| Policy language | Phase 21 |
| Embedding spaces | Phase 22 |
| Server mode | Phase 23 |
| Packaging | Phase 24 |
| Secret stores | Phase 25 |
| Embedder adapters | Phase 26 |
| DataFusion | Phase 27 — last, only if a profile says SQLite is the bottleneck |

**Not phases** (not the product): `agents` table, a second store, graph DB, hosting models, NL2SQL as the product, rewriting `v001.sql`, a cloud control plane.

---

## Phase 17 — RAG citations

**Status: done.** `aidb_generate` / `AI_GENERATE` over `aidb_search` (or a cascaded document set) returns `{ answer, sources[] }` from the retrieval nodes. Plain generate stays a string. No citations table.

**Goal:** When generate uses retrieved context, the answer carries sources. Composition of primitives, not a RAG framework.

**In**

- First-class result shape: `{ "answer": "...", "sources": [{ "document_id", "chunk_id", "score" }] }`
- `aidb_generate` / `AI_GENERATE` over a search or document set writes that JSON (or a pair of columns). Same run, same IR (`Scan` → `Similarity` → `TopK` → `Llm`)
- Sources come from the retrieval nodes already on the plan. Do not invent a citations table

**Out**

- A separate RAG product or citation store
- Knowledge graphs
- Changing generate-without-retrieval (plain prompt → string stays a string)

**SQL when done**

```sql
SELECT aidb_generate('What is the refund policy?', content)
FROM aidb_search('refund policy', 5);
-- answer JSON includes sources[].document_id from those rows
```

**Crates:** `aidb-sql`, `aidb-ai`, `aidb-ir`

---

## Phase 18 — Search metadata filters

**Status: done.** `aidb_search(q, k, filter)` and `SEARCH … WHERE metadata.foo = …` apply JSON metadata on `documents`. Same function, same IR `Filter`. Memory search uses the same path.

**Goal:** Retrieval is embed → search → metadata filter → rank. Hybrid (Phase 10) already exists. Filters do not.

**In**

- `aidb_search(query, k, filter)` (or `SEARCH … WHERE metadata.foo = …`) applies JSON metadata on `documents` after / with KNN + FTS
- Same `aidb_search` name. Same IR (`Filter` on the retrieval plan). Optimizer may push the filter
- Memory search can reuse the same filter path (`metadata.scope` already exists)

**Out**

- A second search function
- Graph / tag databases
- Rewriting `vec_chunks` layout

**SQL when done**

```sql
SELECT document_id, chunk_id, content, distance
FROM aidb_search('refund policy', 5, '{"dept":"support"}');
```

**Crates:** `aidb-index`, `aidb-sql`, `aidb-ir`, `aidb-opt`

---

## Phase 19 — Live MCP client

**Status: done.** `aidb_mcp_connect('stdio', …)` spawns a local MCP server, lists tools, and upserts `capabilities` with `source = 'mcp'`. Invoke still goes through the catalog + deny-list + HITL and writes `kind='tool'` runs. `aidb_mcp_disconnect()` keeps the rows.

**Goal:** MCP is still an adapter into the capability catalog. Phase 13 writes rows. This phase talks to a real MCP server and then writes the same rows.

**In**

- Spawn / connect to an MCP stdio (or local) server, list tools, `INSERT`/`UPDATE` `capabilities` with `source = 'mcp'`
- Invoke through the existing catalog + deny-list + HITL. Tool child runs stay `kind='tool'`
- Disconnect does not delete the catalog rows (they remain until the user drops them)

**Out**

- MCP as a second tool runtime beside the catalog
- Network-open `http.get` as a substitute for MCP
- Tools that skip `runs`

**SQL when done**

```sql
SELECT aidb_mcp_connect('stdio', './fake-mcp');
SELECT name, source FROM capabilities WHERE source = 'mcp';
SELECT aidb_agent('Use the connected MCP tool', '["echo.ping"]');
```

**Crates:** `aidb-tool`, `aidb-sql`

---

## Phase 20 — AI runtime (more providers, classify)

**Status: done.** Anthropic sits behind the same `Llm` trait as fake / OpenAI. `aidb_classify` writes a `kind='generate'` run. No classify store.

**Goal:** The AI runtime stays thin: embed, LLM, then classify (vision later if a provider already has it). Same `models` catalog. Keys still from the environment.

**In**

- At least one more LLM / embed provider behind the same traits (`AIDB_LLM`, `AIDB_EMBEDDER`)
- `classify` as a thin operator or UDF that writes a run (`kind` stays generate / tool — do not add a classify store)
- `CREATE MODEL` already exists; new providers register the same way

**Out**

- Hosting models
- A second generate path that skips `runs`
- Vision as a product (optional later if the provider call is the same trait)

**SQL when done**

```sql
CREATE MODEL IF NOT EXISTS cls PROVIDER 'fake' KIND 'llm';
SELECT aidb_classify('positive or negative', content) FROM documents LIMIT 3;
SELECT aidb_last_run_id();  -- this connection's classify/generate run, not a guess by time
```

**Crates:** `aidb-ai`, `aidb-sql`, `aidb-run` (`aidb_last_run_id`)

---

## Phase 21 — Policy language

**Status: done.** `aidb_set_policy` writes `aidb_meta`. The optimizer and tool runtime read the same object. Goal `CONSTRAINTS` and `AIDB_MAX_*` overlay it (tightest wins). Irreversible tools still HITL. No secrets in the file.

**Goal:** Budget + deny-list + HITL stay. Add a small, declarative policy the optimizer and tool runtime both read. Not a sidecar service.

**In**

- Named rules: allow / deny tools, max $, max ms, read_only, require approval. Stored in the file (additive table or `aidb_meta`), not a second policy DB
- Goal `CONSTRAINTS` and `AIDB_MAX_*` overlay the same policy object
- Irreversible tools still HITL even if policy says allow

**Out**

- A general-purpose policy engine / OPA embed
- Policy that lives only in the process and vanishes on reopen
- Secrets or model keys in the policy table

**SQL when done**

```sql
SELECT aidb_set_policy('{"read_only":true,"deny":["send.email"],"max_usd":0.10}');
SELECT aidb_agent('Email the customer', '["send.email"]');
-- denied or awaiting_approval; file still has the policy after reopen
```

**Crates:** `aidb-tool`, `aidb-run`, `aidb-sql` (additive schema if needed)

---

## Phase 22 — Multiple embedding spaces

**Status: done.** `aidb_create_space` adds a named vec table. Default search still uses `vec_chunks` + `aidb_meta`. No second file. No forced re-embed on open.

**Goal:** One file can hold more than one embedding space. The default space stays what `aidb_meta` already records.

**In**

- Named space: model + dimensions + distance. Additive schema (do not rewrite `vec_chunks` in place — new table or qualified vec table)
- `aidb_search(..., space)` / insert path chooses a space. Documents may exist in one or more spaces
- Optimizer cost still sees the space as a physical bind

**Out**

- A second database file per space
- Breaking the default one-space path
- Re-embedding the world as a required migration
- Local FastEmbed / custom providers (Phase 26). Phase 22 is the catalog + extra vec table

**SQL when done**

```sql
SELECT aidb_create_space('legal', 'fake', 32);
SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal');
```

**Crates:** `aidb-index`, `aidb-storage` (additive migration), `aidb-sql`

---

## Phase 23 — Server mode

**Status: done.** `aidb serve ./app.db` (and `aidb-server`) is HTTP in front of the same `Aidb` and the same file. Optional. The embedded file remains the product. CLI `sql` / bindings do not require the server.

**In**

- `aidb serve ./app.db` (or `aidb-server`) accepts SQL / a small JSON API and returns rows
- One writer still. WAL readers still. Same runs table
- Auth is the application’s job (DESIGN §25). Server may take a bearer from the environment — not users in the file

**Out**

- A hosted control plane
- A second run store “for the server”
- Making the server required for CLI or bindings

**When done**

```bash
aidb serve ./app.db
curl -s localhost:8080/health
curl -s localhost:8080/sql -d "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
```

Optional `AIDB_BEARER` / `AIDB_TOKEN` (Authorization: Bearer). Bind with `--bind 127.0.0.1:8080` or `AIDB_SERVE_BIND` / `AIDB_SERVE_PORT`. POST `/sql` body is raw SQL or `{"sql":"..."}`.

**Crates:** `aidb-server` (new), `aidb-cli`

---

## Phase 24 — Packaging / release

**Status: done.** The TypeScript and Python faces install as `aidb`. The CLI installs as `aidb`. Native addons ship inside the npm package / wheel. No hand-copied dylib.

**In**

- Versioned napi (`aidb.node`) and Python wheel (`aidb_native`) for the platforms we claim
- `aidb` CLI release binary
- README install path that does not say “copy the dylib by hand”

**Out**

- A rewrite of the native addons
- Publishing secrets or fixture API keys
- A separate “cloud package”

**When done:** `npm i aidb` / `pip install aidb` open `./app.db` and `SELECT` `schema_version`.

From this repo (same packages):

```bash
npm i ./bindings/typescript
pip install ./bindings/python
cargo install --path crates/aidb-cli
bash bindings/verify-packaging.sh
```

Prebuilt artifacts (macOS arm64, Linux x64 gnu, Windows x64): `.github/workflows/release.yml`. Does not publish; no registry tokens in the repo.

**Crates:** `aidb-node`, `aidb-python`, `bindings/*`, `aidb-cli`

---

## Phase 25 — Secret stores

**Status: done.** Env is first. `AIDB_SECRET_STORE=keychain` or `file:/path` is optional. `models.key_name` is a name. Reopen without the store is the same missing-key error, not a corrupt file.

**Goal:** Keys still never live in `app.db`. Env remains the default. Optional OS keychain / secret-store lookup by name.

**In**

- Resolve `OPENAI_API_KEY` (and later provider keys) from env first, then an optional store (`keychain`, `file:` outside the db)
- Model catalog may store a **key name**, never the secret
- Reopen without the store → same usage error as missing env, not a corrupt file

**Out**

- Secrets in `app.db`, `aidb_meta`, or `models`
- A required daemon
- Encrypting the whole database as a substitute for this phase

**SQL when done**

```sql
-- models.provider key name only; value comes from env or keychain
SELECT name, provider FROM models;
```

**Crates:** `aidb-ai`

---

## Phase 26 — Embedder adapters (strict spaces)

**Status: done.** There is no “the AIDB embedding.” A space owns `(provider, model, dimensions, distance)`. Search and insert use that space’s embedder. `fake` stays the test default. Local catalog is BGE / Nomic / E5. Custom is an in-process `Embedder`. Weights stay out of the file.

**Goal:** There is no “the AIDB embedding.” An embedding space owns `(provider, model, dimensions, distance)`. A vector index belongs to that tuple. Query and insert use the space’s embedder, not the process-global `open()` embedder.

```text
                Embedder (trait)
                     │
        ┌────────────┼────────────┐
        │            │            │
      local        openai       custom
        │            │            │
    FastEmbed       API        developer
        │
   ┌────┼─────┐
   ▼    ▼     ▼
  BGE  Nomic  E5
```

**In**

- Adapter factory: `local` (FastEmbed: BGE / Nomic / E5), `openai`, `custom`. `fake` stays the test / no-key default
- Search and index resolve the embedder from the `embedding_spaces` row (default space: `aidb_meta`). `AI.open({ embedding })` configures the default space only
- Insert fan-out matches the full tuple (provider + model + dimensions + distance), not dimensions alone
- Distance is declared on the space (`cosine` / `l2`), locked with the vec table at CREATE
- Fail closed on adapter missing, key missing, or tuple mismatch. Do not fall back to OpenAI because local was not loaded
- Documents may exist in many spaces (dual index). One KNN never mixes two functions

**Out**

- Picking BGE, Nomic, or `text-embedding-3-small` as the product default
- Mixing models in one KNN / hybrid-across-spaces
- Model weights in `app.db`
- Auto-selecting a model from the query text
- A plugin ABI or WASM embedder
- Re-embedding the world as a required migration
- DataFusion

**SQL when done**

```sql
SELECT aidb_create_space('legal', 'local', 384, 'BAAI/bge-small-en-v1.5', 'cosine');
SELECT document_id FROM aidb_search('indemnity', 5, NULL, 'legal');
-- query embeds with that space's model. An OpenAI default open() does not leak into legal.
SELECT name, provider, provider_model, dimensions, distance, vec_table FROM embedding_spaces;
```

Changing model, dimensions, or distance is a new space / rebuild, not `ALTER` on `vec0`. Custom is an `Embedder` impl the factory can construct; unknown provider fails like an unknown LLM.

**Crates:** `aidb-ai`, `aidb-index`, `aidb-sql`

---

## Phase 28 — Executable specification (test hardening)

**Status: done.** Runs before Phase 27 even though it is numbered after it: DataFusion stays last. No new architecture. The documented behaviour of the shipped phases is an offline, deterministic test suite, and the bugs it found are fixed with regression tests. `cargo fmt` and `clippy -D warnings` gate CI.

**Goal:** Turn `DESIGN.md`, `PHASES.md`, the schemas, and `examples/sql/` into tests a developer can trust. Answer “can someone actually use AIDB correctly and reliably?” rather than “does it compile?”

**In**

- Contract tests per area, through the surfaces a user has: SQL, Rust API, CLI, HTTP, TypeScript, Python
- Migration tests for v001 → v007, including data written on an old version and read after upgrade
- Crash/resume tests at real checkpoint boundaries, driven by a test-only crash point (`AIDB_TEST_CRASH_POINT`), asserting no lost work and no repeated committed work
- Fail-closed tests for embedding spaces: wrong dimensions, wrong model, missing key, unknown provider, and never a silent fallback to another provider
- Secret tests that scan every table in the file and assert no credential-shaped value was ever stored
- One cross-language test: Rust creates the file, TypeScript writes, Python reads and writes, the CLI reads, Rust verifies documents / vectors / runs / memory / models / capabilities
- `examples/sql/phase*.sql` executed as part of the suite, so a demo cannot rot
- Opt-in live provider smoke tests (`AIDB_LIVE_TESTS=1`), never required

**Out**

- A second storage engine, run store, planner, or schema rewrite
- Public API changes made only to ease testing
- Mock engines behind the bindings (the bindings load the real addon)
- Live credentials in the normal suite
- Test-only crash behaviour in a release build

**Bugs this phase fixed**

- Updating `documents.content` did not re-index (added `schema/v007.sql` reindex trigger; chunks and vectors are rebuilt)
- Deleting a document left orphan vector rows (delete trigger per space vec table, plus a one-time prune on open)
- Invalid `metadata_json` was accepted and stored
- Empty or whitespace-only content produced an unrankable zero vector that broke search
- Hybrid RRF ties depended on hash order, so the same query could return a different order
- A space could be recorded even when its embedder could not be constructed (creation is now pre-validated and transactional)
- `generate` / `classify` over a search ignored the named space
- Workflow JSON could not express a `tool` step, so a reachable operator had no frontend
- A negative literal argument parsed as unknown SQL instead of an out-of-range value
- A `k` larger than the `vec0` KNN limit failed instead of being bounded by the corpus

**Commands**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace        # cargo test does not refresh the CLI the E2E tests spawn
cargo test --workspace
cd bindings/typescript && npm run build && AIDB_CLI_BIN=… node test.mjs
cd bindings/python && python3 scripts/stage_native.py && AIDB_CLI_BIN=… python3 test_open.py
AIDB_PACKAGING_TESTS=1 cargo test -p aidb --test packaging
AIDB_LIVE_TESTS=1 OPENAI_API_KEY=… cargo test -p aidb --test live_providers
```

**Crates:** every crate (tests), plus fixes in `aidb-core`, `aidb-storage`, `aidb-index`, `aidb-sql`, `aidb`

---

## After 28 — how a new primitive gets in

Do not start from “LangGraph has X.” Start from an AIDB-only app. When you hit `I need X`:

> Should X become an AIDB primitive, or does it belong to the application?

| Problem | AIDB? |
| --- | --- |
| Persist research documents | **Yes** |
| Semantic search | **Yes** |
| Embedding lifecycle | **Yes** |
| AI execution | **Yes** |
| Durable runs | **Yes** |
| Resume after crash | **Yes** |
| Workflow execution | **Yes** |
| Cost-aware optimization | **Yes** |
| Tool execution / policy | **Yes** |
| Agent execution | **Yes** |
| Chat UI | Application |
| Authentication | Application |
| Domain UI | Application |
| Broker / Slack / HTTP APIs | Tool or provider |
| PDF parser | Application or provider |
| Prompt editor UI | Application / Studio |

Phases 29–31 are the platform. 32–35 shipped because the desk needed them, as file-shaped primitives, not a graph library. `aidb_last_run_id()` and parked `output_json` as JSON followed the same rule. Do not rebuild LangChain inside SQLite.

---

## Phase 29 — Stock application (AIDB-only)

**Status: done.** `examples/stock` is an equity research desk with no AI framework behind it: filings, embeddings, cited answers, agent steps, approvals, spend, and the desk's own `watchlist` / `signals` tables in one file. No LangChain, no LangGraph, no LangSmith, no vector service, no trace backend.

**Goal:** build one real app, discover what sucks, and only add an engine primitive when a logged gap belongs in the file.

**Shipped**

- `examples/stock/stock.mjs` — `init`, `ingest`, `ask` (with `--ticker` / `--user`), `remember`, `brief`, `digest`, `sentiment`, `waiting`, `approve` / `reject`, `runs`, `status`, `demo`. Default path is offline and deterministic; `--live` uses a real provider
- `examples/stock/NOTES.md` — the “I needed X” log, decision by decision
- `examples/sql/phase29_stock.sql` — the same desk in SQL only
- `crates/aidb/tests/stock_app.rs` — the contract the app depends on, plus the app itself run end to end through node and the napi addon
- `.github/workflows/ci.yml` — fmt, clippy `-D warnings`, build, `cargo test --workspace`, both binding suites, the app demo, and Studio, on Linux and macOS, offline

**Found and fixed in the engine**

- A projected retrieval ignored its column list: `SELECT document_id, content FROM aidb_search(...)` returned all four columns, so a caller reading by position got `chunk_id` where it asked for `content`. Our own memory face did exactly that. `*` and expressions still return the whole row; an unknown column is now an error
- An approved agent asked again: the model says DONE while drafting, the email tool ran after it and its output erased the signal, so the loop re-parked and the approval queue never drained. Every existing HITL test used a single-tool, single-step agent, so nothing caught it
- A scalar AI function did not name its run: `aidb_classify` returns a label, so the desk guessed the newest generate by time. `SELECT aidb_last_run_id()` is this thread's last insert
- Parked `output_json` was not always JSON: an agent stored a plain approval message. `park_run` now stores `{"paused","status","message"}`; the SQL `output` column stays the human text

**Closed in later phases (logged from this desk, not a graph library)**

- A workflow `approve` then irreversible `tool` failed on resume — Phase 33 honors the prior approve
- An agent could not pass tool arguments or scope search — Phase 33 `"decide":true`
- Turn 1 / 2 / 3 needed a thread of runs — Phase 34 `session_id` + views
- A reconnect needed the generate prefix — Phase 35 `run_events` kind=`token`

**Never needed:** a conversations table, a second store, an `agents` table. Sessions, decide, structured generate, and streaming landed as file-shaped primitives (Phases 32–35), not as a graph library.

**SQL when done**

```sql
SELECT aidb_insert_document('AAPL 10-K excerpt', '…', '{"ticker":"AAPL"}');
SELECT aidb_generate('Answer only from the sources', content)
  FROM aidb_search('what is the margin guidance', 3, '{"ticker":"AAPL"}');
SELECT aidb_agent('{"instructions":"Draft the digest, then email it. End with DONE.","goal":"Morning digest for NVDA","tools":["search","generate","send.email"],"max_steps":4}');
SELECT aidb_resume('run_…', '{"approved":true}');
SELECT id, kind, parent_id, status, cost_usd FROM runs ORDER BY created_at_ms;
```

**Crates:** `examples/`, `.github/workflows/`, plus the two engine fixes in `aidb-sql` and the `aidb` agent loop

---

## Phase 30 — Studio as inspect face

**Status: done.** Studio is a face over `aidb serve`. Same file. Not a second engine. Not the chat product. Built after Phase 31 so the inspect face includes `experiment_results`.

**Goal:** A developer can see documents, search, runs (including waiting), models, experiments, and SQL without leaving the browser.

**What shipped**

- Pages that are `SELECT`s, from one catalog (`studio/src/lib/catalog.mjs`): file/meta, documents, `aidb_search`, runs, models, `experiment_results`, `sessions` / `session_turns`, generate tokens on the latest generate run
- Waiting-run badge is `COUNT(*) FROM runs WHERE status = 'awaiting_approval'`. Approve / reject is `SELECT aidb_resume(id, '{"approved":…}')` from the peek pane — still `/sql`
- Bearer for a protected serve: `Authorization: Bearer` on `/sql` and `/health`, `?token=` on `/ws` (browsers cannot set that header on a WebSocket). Stored in this browser, or injected by Vite from `AIDB_BEARER`. No users table. Loopback by default
- Live catalog via `GET /ws` on the same process

**Out**

- Auth product, users table, chat product, a trace warehouse
- Studio spawning `aidb sql` instead of `/sql`
- Prompt editor as a core table (Phase 29 did not ask for one; the SQL console is the editor)

**SQL when done**

```sql
SELECT key, value FROM aidb_meta ORDER BY key;
SELECT plan, accuracy, cost_usd FROM experiment_results;
SELECT aidb_resume('run_…', '{"approved":true}');
```

**Crates:** `studio/`, `aidb-server` (WS query-token already existed; Studio now sends it)

---

## Phase 31 — Experiments / evals in the file

**Status: done.** The optimizer’s claim is now a row. Ran before Studio because it is engine work and Studio should be built once, over the richer file.

**Goal:** Plan A vs Plan B is a queryable object: dataset, two plans, cost, latency, quality. The optimizer stops being a claim.

**What shipped**

- `eval_examples` is a table you `INSERT` into. Gold is the text an answer must contain, the documents retrieval must find, or both — never neither (a `CHECK`, so an ungradeable example cannot be stored)
- `aidb_experiment('{"dataset":…,"plans":["naive","cascade"],"k":3}')` runs every example through every named plan under the same policy budget. Named plans, not SQL templates: `naive` (one model call per document, deliberately unrewritten), `cascade` (retrieve top k, then answer), `search` (retrieval only — the price floor)
- The comparison is a run, each plan is its child, and the leaf `generate` / `search` runs are parented to the plan, so a plan’s `cost_usd` is its children’s spend rolled up. `experiment_results` is a **view** over `runs` — no experiment store
- The verdict is in the file: `$.best.plan` is highest accuracy, then lowest cost, among plans that answer. Retrieval-only is free and answers nothing, so it is never allowed to win
- A plan that cannot fit the budget **fails as a row**, with the reason (`budget exceeded: 2 LLM calls > max_llm_calls=1`), while the other plans still run. That finding belongs in the file, not in an exception the caller catches
- An experiment interrupted mid-flight is recovered to `failed` on open instead of staying `running` forever

**What it proves**

On a 9-document corpus with one labeled question, both plans answer correctly, and `cascade` does it in 1 model call for about a third of the money — while a `max_llm_calls=1` budget kills `naive` outright and `cascade` still answers. Where retrieval cannot reach the gold, `cascade` scores `recall=0`, `accuracy=0` and `naive` wins on quality: the trade is visible in both directions.

**Out**

- LangSmith clone, hosted eval UI as the identity
- Static “95% quality” numbers on operators
- A second run store for experiments

**SQL when done**

```sql
INSERT INTO eval_examples (dataset, question, expect_text)
VALUES ('support_gold', 'how long do refunds take', '14 days');

SELECT aidb_experiment('{"dataset":"support_gold","plans":["naive","cascade"],"k":3}');
SELECT plan, examples, accuracy, recall, llm_calls, cost_usd, latency_ms, status
  FROM experiment_results ORDER BY cost_usd;
```

**Gaps logged, not built**

- A cached model call is not charged against `max_llm_calls`, so a warm cache can make an over-budget plan look affordable. An experiment reports the calls the plan *makes*, which is why the budget contract is tested on a cold file
- Plans are a fixed registry. A user-defined plan means executing arbitrary SQL per example, which is a bigger surface (and a templating story) than this phase needed
- Quality is substring-and-citation grading. LLM-as-judge would be another model call to price, and it is not needed to price a rewrite

**Crates:** `aidb-sql` (spec + plan execution), `aidb` (`experiment.rs`), `aidb-run` (roll-up + recovery), `schema/v008.sql` (`experiment` run kind, `eval_examples`, `experiment_results` view)

---

## Phase 32 — Structured generate

**Status: done.** The stock desk could not pass tool args or reliably structure a classification. Generate and classify now take a JSON schema; invalid output fails the run — a row, not an exception type the app has to parse.

**Goal:** A third argument is a JSON Schema. Matching output is canonical JSON. A mismatch is `runs.status = 'failed'` with the raw text still on the row, so the next operator (Phase 33’s decide step) can demand a tool name and args without hoping the model punctuated them.

**What shipped**

- `SELECT aidb_generate(prompt, content, schema)` and `SELECT aidb_classify(labels, content, schema)`. Two-arg calls are unchanged. `ai_generate` has the same third argument
- The schema is a JSON object (a JSON Schema subset: `type`, `enum`, `const`, `properties` / `required`, `items`, `minimum` / `maximum`). Junk schema JSON is a usage error **before** a run is opened
- Invalid **model** output fails the generate run: `status = 'failed'`, `error` like `output did not match schema: …`, `output_json` keeps the raw `text` plus `schema_error`, spend is still recorded. The SQL statement errors with the same message so the app inspects `runs` rather than exception types
- The cache key includes the schema. Invalid output is not cached. A typed call is not a replay of an untyped one
- The fake LLM fills JSON from a prompt marker (`AIDB_JSON_SCHEMA:`) so tests stay offline. Live providers get the same suffix

**What it proves**

A required `summary` string comes back as `{"summary":"…"}` with `status = 'succeeded'` and the schema on `input_json`. A required `const` the content cannot support errors the statement, leaves that run `failed` with the raw text, and does not open a run at all when the schema itself is not JSON. Classify with `{"enum":["positive","negative"]}` returns a JSON string in the set, and a label outside it fails the run. `FROM documents` and `FROM aidb_search` take the same third argument.

**Out**

- Prompt Hub, prompt versioning as a product (that is Studio / application)
- A JSON Schema crate or `$ref` catalogue. The subset lives in `aidb-sql`

**SQL when done**

```sql
SELECT aidb_generate('Summarize this', content);
SELECT aidb_generate(
  'Extract a summary',
  content,
  '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'
);
SELECT aidb_classify(
  'positive or negative',
  content,
  '{"enum":["positive","negative"]}'
);
SELECT status, error, json_extract(output_json, '$.schema_error')
  FROM runs WHERE kind = 'generate' ORDER BY created_at_ms DESC LIMIT 1;
```

**Gaps logged, not built**

- Live providers are not forced into a native `response_format` / tool-call API; they see the schema as prompt text. A provider that ignores it still fails the run when the output does not match
- The subset does not implement `$ref`, `oneOf`, or `additionalProperties: false`. Enough for tool args and labels; not a schema catalogue
- Phase 33’s decide operator uses this schema so a tool name and args are a contract, not punctuation the model hoped to get right

**Crates:** `aidb-sql` (`schema.rs` + 3-arg UDFs + `generate_task`), `aidb-ai` (fake filler + prompt marker)

---

## Phase 33 — Dynamic agent

**Status: done.** The stock desk could not pass tool arguments or scope a search, and a workflow that approved an irreversible tool still failed on resume. `"decide":true` makes each agent step a schema-valid choice; workflows honor a prior `approve`. Recipe agents (`tools` in order until DONE) stay the default. Still no `agents` table.

**Goal:** The model chooses among catalog tools / search / generate / stop, with arguments, as child runs under the same policy, HITL, and file.

**What shipped**

- `SELECT aidb_agent('{"decide":true,"tools":["search","generate","send.email"],…}')`. Each iteration is a structured generate (`op` + `args`) then the chosen operator. Checkpoints are `a.{step}.decide` then `a.{step}.{op}`
- Search args may include `query`, `k`, a metadata `filter`, and `space`. Catalog tools receive the JSON `args` the model produced — `send.email` is no longer hardcoded to `user@example.com`
- `max_steps` still bounds the loop. `stop` ends it. Invalid decide output fails the generate child and the agent, same as Phase 32
- HITL is unchanged: an irreversible tool parks, `aidb_resume` invokes with the **stored** args
- A workflow `{"then":[{"approve":…},{"tool":"send.email"}]}` now runs the tool after approval. A tool node with no prior approve still fails closed
- `EXPLAIN` prints `Decide tools=… max=…`. Bindings pass `decide: true`. The stock digest uses it

**What it proves**

A decide agent with `search` + `generate` records decide / search / generate children and stops instead of replaying search to burn `max_steps`. A goal like `Brief me on NVDA only` puts a ticker filter on the search run. `Email alice@desk.test a refund summary` parks, and after approval the tool output contains that address, not `user@example.com`. An approved digest still finishes once.

**Out**

- LangGraph graphs as the identity
- Unconstrained loops with no `max_steps` / budget
- Replacing workflows — workflows stay the static path
- An `agents` table

**SQL when done**

```sql
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":4,"decide":true}');
SELECT node_id FROM checkpoints WHERE run_id = 'run_…' ORDER BY node_id;
SELECT aidb_workflow('{"then":[{"search":{"query":"refunds","k":3}},{"approve":{"message":"send?"}},{"tool":"send.email"}]}');
SELECT aidb_resume('run_…', '{"approved":true}');
```

**Gaps logged, not built**

- Recipe agents still hardcode catalog-tool args. Decide is the path that chooses them
- The fake LLM’s decide policy is “first unused allowed op, then stop.” A live model can choose a different order; `max_steps` and HITL still apply

**Crates:** `aidb` (`agent.rs`, `workflow.rs`), `aidb-ir` (`LogicalOp::Decide`), `aidb-ai` (fake decide filler), `aidb-sql` (`AgentSpec.decide`)

---

## Phase 34 — Sessions

**Status: done.** Memory-as-documents stays the default. A thread is a view over runs, not a second memory store. Turn 1 / 2 / 3 is a `SELECT`.

**Goal:** Stamp a thin `session_id` on runs so a chat-shaped app can read a thread without minting a messages table or replacing `memory`.

**What shipped**

- `ALTER TABLE runs ADD COLUMN session_id TEXT` (nullable). Old rows stay `NULL`. There is no `sessions` table
- `SELECT aidb_session('desk:nvda')` binds this connection; `SELECT aidb_session()` returns the current bind; `SELECT aidb_session(NULL)` clears it. An empty name is a usage error. Later generate / search / classify / tool / workflow / agent inserts on that connection stamp `session_id`. Indexing does not
- `SELECT aidb_agent('{"session":"desk:nvda",…}')` (and the same field on a workflow spec) stamps the parent even without a prior bind. Children inherit the parent’s `session_id`
- `sessions` and `session_turns` are **views**. `session_turns` is top-level runs (`parent_id IS NULL`) with a `turn` number. Memory is still documents
- Bindings pass `session` through. Studio’s catalog can `SELECT` the views; it did not grow a chat page

**What it proves**

Two generates after `aidb_session('desk:nvda')` are turn 1 and turn 2. A second session does not leak. An agent with `"session":"chat-1"` stamps the parent; every child has the same `session_id`; children are not turns. Unscoped generate still has `NULL`. Migrating a v008 file keeps legacy runs and adds the column as `NULL`.

**Out**

- ChatGPT clone, message table as a new engine
- Replacing the memory view
- A minted `sess_` table

**SQL when done**

```sql
SELECT aidb_session('desk:nvda');
SELECT aidb_generate('What is NVDA?', 'Data center revenue was 47.5 billion.');
SELECT aidb_generate('And the risk?', 'Supply concentration in Taiwan.');
SELECT turn, kind, json_extract(input_json, '$.prompt') FROM session_turns WHERE session_id = 'desk:nvda' ORDER BY turn;
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"session":"desk:nvda"}');
SELECT * FROM sessions;
```

**Gaps logged, not built**

- The bind is thread-local (this connection / this process), like the job meter. HTTP one-statement-at-a-time should pass `"session"` on the agent. The file stores the stamp, not the bind
- Documents are not tagged. Search/memory stay scoped by metadata as before

**Crates:** `aidb-run` (`session_id` at insert), `aidb-sql` (`aidb_session`, `AgentSpec.session`), `schema/v009.sql` (column + views)

---

## Phase 35 — Streaming

**Status: done.** Tokens append to the generate run as events. A reconnect still has the prefix. HTTP and bindings read those rows. There is no second generate path.

**Goal:** Stream only as durable `run_events` on the same generate run that `aidb_generate` already opens.

**What shipped**

- Fake generate splits the completion into chunks and `append_token` writes `kind = 'token'` with `{"text":…}`. Concatenating `$.text` in `seq` order is the prefix. Live OpenAI / Anthropic stream SSE deltas into those same token events; adapters that do not override `complete_streaming` still emit one token (the full text)
- A cache hit does not stream: it stays `cache_hit` on the run
- Crash after the first token leaves the prefix on `run_events`. Reopen marks the run `failed` / `interrupted`; `output_json` is still empty. The prefix is the events
- `GET /runs/{id}/events` returns those rows. WebSocket publishes `{"type":"token","run_id","seq","text"}` as they are written. Bindings: `db.runs.tokens(id)` / `events(id)`
- Studio treats a token frame like a catalog change so the run peek (already a `SELECT` from `run_events`) refreshes. No second store

**What it proves**

`SELECT aidb_generate(…)` still returns the full text. That run has `started`, then more than one `token`, then `generated`. The tokens concatenate to the SQL result. A second identical call is a cache hit with no tokens. HTTP GET after the fact returns the same events. A `kill -9` after the first token still has the prefix in the file.

**Out**

- Streaming as a second generate path that skips runs
- Mid-token checkpoints (DESIGN: checkpoint after the operator, not mid-token)
- Provider-native SSE as the identity. The file is the identity

**SQL when done**

```sql
SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.');
SELECT seq, kind, json_extract(payload_json, '$.text') AS text
  FROM run_events
 WHERE run_id = aidb_last_run_id()
 ORDER BY seq;
```

A new `aidb sql` process starts with an empty last-run bind; then name the run from `runs` as before. HTTP: `GET /runs/{id}/events`. Live OpenAI / Anthropic stream SSE deltas into the same token events; the file is still the identity.

**Gaps logged, not built**

- The write mutex still holds the connection for the whole generate. Live UI is the WebSocket publish, not a second reader on the same `Aidb`

**Crates:** `aidb-ai` (`complete_streaming`; fake chunks; OpenAI / Anthropic SSE), `aidb-run` (`append_token` + listeners), `aidb-sql` (generate writes tokens), `aidb-server` (`GET /runs/{id}/events` + WS)

---

## Phase 27 — DataFusion (only if needed)

**Status: only if needed. Last.**

**Goal:** Swap the data runtime when SQLite is the measured bottleneck. Not a product phase. Do not start this because the crate list looks incomplete.

**In**

- Physical bind already says SQLite vs AI. Add DataFusion as another data backend
- Same IR, same runs, same file story (or an explicit export — do not fork the product)
- Profile first: a query that is slow because of SQL, not because of LLM

**Out**

- Replacing SQLite as the default
- A second WAL / run implementation
- Starting this phase to “modernize” the stack

**Crates:** `aidb-storage` (or a new data runtime crate), `aidb-opt`

---

## 4. What each phase must not do

| Temptation | Why not |
| --- | --- |
| TS/JS SDK in Phase 0–3 | We are proving SQL and the file |
| `SEARCH` / `CREATE MODEL` parser in Phase 1 | Functions first, dialect later |
| DataFusion | SQLite until a profile says otherwise (Phase 27, last) |
| LangGraph clone | Persistence is the file. Do not win on agents. |
| Workflow tables in v001 | Compile to runs + checkpoints |
| Secrets in `app.db` | Environment (then optional store). Never the file |
| Agents as the first abstraction | Q14 |
| Optimizer before search feels good | Q3 |
| A second AI path for SQL UDFs | Everything goes through runs |
| Live MCP before the catalog | Phase 13 first; Phase 19 talks to MCP and writes the same rows |

---

## 5. Suggested first files

Phase 0 (done):

```text
crates/aidb-core/src/lib.rs
crates/aidb-storage/src/lib.rs      open, pragma, migrate
crates/aidb/src/lib.rs              Aidb { execute, query }
crates/aidb-cli/src/main.rs         aidb sql <db> <sql>
schema/v001.sql                     already exists
tests/sql/phase0_meta.sql
```

Phase 1 (done) added:

```text
crates/aidb-index/src/{chunk,index,status}.rs
crates/aidb-ai/src/{embed,provider}.rs
crates/aidb-sql/src/search.rs
crates/aidb-run/src/lib.rs
examples/sql/phase1_search.sql
```

Phase 9 (done) added `schema/v002.sql` (`awaiting_approval` / `suspended`) plus `aidb_resume`. Do not rewrite `v001.sql`.

```text
schema/v002.sql                     awaiting_approval / suspended (if needed)
crates/aidb-run/src/resume.rs
crates/aidb-sql/                    aidb_resume
crates/aidb-cli/                    aidb runs --waiting
examples/sql/phase9_hitl.sql
```

Phase 12 (done) replaced the thin CLI/ctypes faces with in-process addons. Same `AI.open` API.

```text
crates/aidb-node/                   napi addon (cdylib)
crates/aidb-python/                 PyO3 module aidb_native (cdylib)
bindings/typescript/src/index.mjs   loads aidb.node, no child_process
bindings/python/aidb/__init__.py    loads aidb_native, no ctypes
examples/sql/phase12_native.sql
```

Phase 13 (done) added the capability catalog. MCP writes rows; agents invoke through policy.

```text
schema/v003.sql                     capabilities + runs.kind = 'tool'
crates/aidb-tool/src/lib.rs         catalog, deny-list, handlers, MCP register
crates/aidb/src/agent.rs            allow-list + HITL for irreversible tools
examples/sql/phase13_tools.sql
```

Phase 14 (done) added shared memory as documents and multi-agent as child runs.

```text
schema/v004.sql                     memory view over documents
crates/aidb/src/memory.rs           aidb_memory_insert / aidb_memory_search
crates/aidb/src/agent.rs            memory scope + child agent runs
examples/sql/phase14_memory.sql
```

Phase 15 (done) added dialect syntax that lowers to the existing functions.

```text
crates/aidb-sql/src/dialect.rs      SEARCH / CREATE MODEL
examples/sql/phase15_dialect.sql
```

Phase 16 (done) added the goal language frontend. It emits IR; it does not skip the optimizer.

```text
crates/aidb-ir/src/goal.rs          GoalSpec → workflow / generate IR
crates/aidb-sql/src/goal.rs         TASK / DATA / CONSTRAINTS / GOAL parser
crates/aidb/src/goal.rs             persist as workflow run
examples/sql/phase16_goal.sql
```

Phase 17 (done) added first-class RAG citations on generate-over-search. Sources come from the retrieval nodes.

```text
crates/aidb-ir/src/lib.rs           LogicalPlan::generate_over_search
crates/aidb-sql/src/lib.rs          cite_answer / parse FROM aidb_search
crates/aidb-sql/src/plan.rs         execute_rag_generate
examples/sql/phase17_citations.sql
```

Phase 18 (done) added metadata filters on the existing search path. Same `aidb_search`. Same IR Filter.

```text
crates/aidb-ir/src/lib.rs           LogicalPlan::search_filtered
crates/aidb-index/src/lib.rs        json_extract on documents.metadata_json
crates/aidb-sql/src/dialect.rs      SEARCH … WHERE metadata.foo
crates/aidb-opt/src/lib.rs          PushFilterBeforeExpensive + MetadataFilter
examples/sql/phase18_filter.sql
```

Phase 19 (done) added a live MCP stdio client. It writes the same catalog rows. It is not a second runtime.

```text
crates/aidb-tool/src/mcp.rs         stdio JSON-RPC client
crates/aidb-tool/src/bin/fake-mcp.rs  local fixture, no network
crates/aidb-sql/src/lib.rs          aidb_mcp_connect / disconnect
examples/sql/phase19_mcp.sql
```

Phase 20 (done) added Anthropic behind the same LLM trait and a thin classify UDF. Classify writes generate runs. No classify store.

```text
crates/aidb-ai/src/lib.rs           AnthropicLlm + Llm::classify
crates/aidb-sql/src/lib.rs          aidb_classify UDF
crates/aidb-sql/src/dialect.rs      CREATE MODEL IF NOT EXISTS + default model
crates/aidb/tests/last_run.rs       aidb_last_run_id is this thread's last insert
examples/sql/phase20_classify.sql
```

Phase 21 (done) added a declarative policy in the file. Not a sidecar. Not a second policy DB.

```text
crates/aidb-tool/src/policy.rs      parse, persist, overlay
crates/aidb-sql/src/plan.rs         optimizer reads the same object
crates/aidb/src/tool.rs             aidb_set_policy / aidb_get_policy
examples/sql/phase21_policy.sql
```

Phase 22 (done) added named embedding spaces in the same file. Default `vec_chunks` is unchanged.

```text
schema/v005.sql                     embedding_spaces catalog
crates/aidb-index/src/space.rs      create, backfill, qualified vec table
crates/aidb-ir/src/lib.rs           space as a physical bind
examples/sql/phase22_spaces.sql
```

Phase 23 (done) added optional HTTP over the same file. Not a control plane. Not a second run store.

```text
crates/aidb-server/src/lib.rs       POST /sql + GET /health
crates/aidb-cli/src/main.rs         aidb serve
examples/sql/phase23_serve.sql
```

Phase 24 (done) packaged the faces. Native addons ship in the npm tarball / Python wheel. CLI is `aidb`.

```text
README.md                           npm i / pip install / cargo install
bindings/typescript/                aidb.node staged into the package
bindings/python/                    wheel with aidb_native
.github/workflows/release.yml       claimed-platform artifacts
examples/sql/phase24_packaging.sql
```

Phase 25 (done) added env-then-store key lookup. The catalog stores a key name. Never the secret.

```text
schema/v006.sql                     models.key_name + reject-secret triggers
crates/aidb-ai/src/secrets.rs       env, then keychain or file:
crates/aidb-sql/src/dialect.rs      CREATE MODEL KEY_NAME
examples/sql/phase25_secrets.sql
```

Phase 26 (done) made the space own the embedder. No process-global “AIDB embedding.” Local catalog is BGE / Nomic / E5. Custom is in-process. Weights stay out of the file.

```text
crates/aidb-ai/src/embed.rs         fake / openai / local / custom factory
crates/aidb-index/src/space.rs      bind from the space tuple
examples/sql/phase26_spaces.sql
```

Phase 28 (done) turned the documented behaviour into an offline suite, and fixed what it caught (content updates now re-index, deletes leave no orphan vectors, spaces fail closed, RRF ties are stable).

```text
schema/v007.sql                     reindex trigger on content change
crates/aidb/tests/                  contracts per area + crash/resume + cross-language
crates/aidb-server/tests/http.rs    HTTP face over the same file
bindings/*/test*                    the real addon, not a mock
.github/workflows/ci.yml            fmt --check + clippy -D warnings + test
```

Phase 31 (done) made the optimizer’s claim a row: a labeled dataset in the file, named plans run under one budget, and results as a view over the runs that produced them.

```text
schema/v008.sql                     experiment run kind + eval_examples + experiment_results view
crates/aidb/src/experiment.rs       run each plan per example, grade against gold, roll up spend
crates/aidb/tests/experiments.rs    cheaper-at-equal-quality as data, budget parity, durability
examples/sql/phase31_experiment.sql
```

Phase 30 (done) made Studio the inspect face over `aidb serve`: the pages are SELECTs, approve is `aidb_resume`, experiments are a view, and a bearer is a header — not a users table.

```text
studio/                             Vite face; POST /sql + GET /ws
studio/src/lib/catalog.mjs          the SELECTs the pages run
crates/aidb/tests/studio.rs         those SELECTs as contracts
examples/sql/phase30_studio.sql
```

Phase 32 (done) made generate/classify take a JSON schema. Invalid output fails the run. Two-arg calls are unchanged.

```text
crates/aidb-sql/src/schema.rs       JSON Schema subset; invalid output is a run failure
crates/aidb-ai/src/lib.rs           fake filler from AIDB_JSON_SCHEMA marker
crates/aidb/tests/structured.rs     canonical JSON, failed run, no run on junk schema
examples/sql/phase32_structured.sql
```

Phase 33 (done) made the agent a decide loop: the model chooses the next operator and its arguments. Recipe agents stay the default. Workflows honor a prior approve. Still no `agents` table.

```text
crates/aidb/src/agent.rs            decide loop; stored args on HITL resume
crates/aidb-ir/src/lib.rs           LogicalOp::Decide
crates/aidb/tests/decide.rs         filter, recipient, stop, EXPLAIN
examples/sql/phase33_decide.sql
```

Phase 34 (done) made a session a thread of runs. Turn 1 / 2 / 3 is a view. Memory stays documents. Still no sessions table.

```text
schema/v009.sql                     session_id on runs; sessions + session_turns views
crates/aidb-run/src/lib.rs          bind + stamp at insert; children inherit
crates/aidb/tests/sessions.rs       two turns, isolation, agent inherit, no table
examples/sql/phase34_session.sql
```

Phase 35 (done) made generate tokens durable events on the same run. A reconnect still has the prefix. HTTP and bindings read those rows.

```text
crates/aidb-ai/src/lib.rs           complete_streaming; fake chunks; OpenAI / Anthropic SSE
crates/aidb-run/src/lib.rs          append_token + listeners
crates/aidb-server/src/lib.rs       GET /runs/{id}/events; WS type=token
crates/aidb/tests/streaming.rs      concat = output; cache hit is silent
examples/sql/phase35_stream.sql
```

Remaining phases (do not implement until started):

```text
Phase 27  data runtime                DataFusion only if profiled
```

---

## 6. Phase map (one screen)

| Phase | Name | Status | User-visible proof |
| --- | --- | --- | --- |
| 0 | Open + migrate | **done** | `SELECT` schema_version |
| 1 | Docs + search | **done** | `aidb_search(...)` |
| 2 | Generate | **done** | `aidb_generate(...)` + `runs` |
| 3 | Durable runs | **done** | resume + `SELECT` from `runs` |
| 4 | IR | **done** | `aidb_explain` |
| 5 | Workflow | **done** | declared graph → child runs |
| 6 | Optimizer | **done** | Plan B on a small labeled set |
| 7 | Bindings | **done** | `AI.open` / Python (thin faces) |
| 8 | Agents | **done** | `aidb_agent` → child runs |
| 9 | HITL | **done** | `awaiting_approval` + `aidb_resume` |
| 10 | Hybrid search | **done** | FTS + vec, one `aidb_search` |
| 11 | Optimizer at scale | **done** | labeled gold cheaper than naive; $ / ms enforced |
| 12 | Native bindings | **done** | napi / PyO3, no CLI spawn |
| 13 | Tools + MCP | **done** | capability catalog, tool child runs |
| 14 | Memory + multi-agent | **done** | memory view, child `agent` runs |
| 15 | SQL dialect | **done** | `SEARCH` / `CREATE MODEL` → same IR |
| 16 | Goal language | **done** | `TASK` / `GOAL` → IR, workflow run |
| 17 | RAG citations | **done** | generate answer + `sources[]` |
| 18 | Metadata filters | **done** | `aidb_search(q, k, filter)` |
| 19 | Live MCP client | **done** | stdio MCP → `capabilities` |
| 20 | AI runtime | **done** | Anthropic + `aidb_classify` → generate run |
| 21 | Policy language | **done** | `aidb_set_policy` in the file |
| 22 | Embedding spaces | **done** | `aidb_search(..., space)` |
| 23 | Server mode | **done** | `aidb serve` over the same file |
| 24 | Packaging | **done** | `npm i` / `pip install` / CLI open the file |
| 25 | Secret stores | **done** | env, then optional keychain / `file:` |
| 26 | Embedder adapters | **done** | space owns model; local / openai / custom |
| 28 | Executable specification | **done** | `cargo test --workspace` + fmt/clippy + binding, CLI, HTTP, crash and cross-language suites |
| 29 | Stock application | **done** | `examples/stock` desk + CI; `aidb_last_run_id`; no Lang* runtime |
| 31 | Experiments / evals | **done** | `SELECT plan, accuracy, cost_usd FROM experiment_results` |
| 30 | Studio inspect face | **done** | documents / search / runs / experiments / tokens over `aidb serve` |
| 32 | Structured generate | **done** | schema-valid generate; mismatch fails the run |
| 33 | Dynamic agent | **done** | decide → child runs; still no agents table |
| 34 | Sessions | **done** | `session_turns` is Turn 1 / 2 / 3 over `runs` |
| 35 | Streaming | **done** | tokens as `run_events`; reconnect has the prefix |
| 27 | DataFusion | only if needed | profile says SQLite is the bottleneck |

v0 is Phases 0–8. Phases 9–26 are done, Phase 28 tests them, Phase 29 proved them with an AIDB-only application, Phase 31 priced the optimizer against labeled data, Phase 30 is the inspect face over that file, Phase 32 made generate/classify take a JSON schema so invalid output fails the run, Phase 33 made the agent a decide loop, Phase 34 made a session a thread of runs, and Phase 35 made generate tokens durable events. The desk also needed `aidb_last_run_id()` and parked `output_json` as JSON — file-shaped leftovers, not a new phase. Phase 27 is DataFusion, last, only if needed. Do not add an `agents` table. Do not invent a second store. Do not rebuild LangChain inside SQLite.
