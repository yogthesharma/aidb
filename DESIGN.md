# AIDB

An embedded database for AI applications.

LangChain and LangGraph help you build AI applications. AIDB is the persistent AI application runtime and database those applications can run on.

Architecture and product boundary live here. Build order (what shipped, what remains): [`PHASES.md`](PHASES.md). Remaining numbered product work is DataFusion, last, only if a profile says SQLite is the bottleneck.

The developer face is one file:

```ts
import { AI } from "aidb";

const db = await AI.open("./app.db");
```

That file holds application data, documents, embeddings, model catalog, retrieval, and durable AI execution. TypeScript, Python, CLI, and optional HTTP are faces over the same engine. The engine inside the file can plan, optimize, persist, and resume mixed data / AI / tool workloads.

AIDB is a database first. The optimizer is a native engine component. Agents are composition on top, not the core abstraction.

---

## 1. What this is

**One-line product**

> A DuckDB/SQLite-like embedded database where AI data and AI execution are native, persistent, queryable, and optimizable.

**One-line execution model**

> A declarative engine whose IR and optimizer understand data operations, reasoning, and actions as different kinds of computation — and whose run is a durable database object.

**What the developer should not have to assemble**

```text
Postgres + Redis + vector DB + LangGraph + LLM SDK
+ checkpoint store + observability
```

They should own one file:

```text
my-app/
├── app.db
└── src/
```

`app.db` is portable: copy it, back it up, move it, put it on object storage. The developer owns the file. No server is required. Optional `aidb serve` and Studio inspect the same file.

---

## 2. What this is not

Do not build:

- another vector database as the product
- a LangChain / LangGraph clone. Do not win on “better agents.” Persistence is the product boundary: the database file *is* the execution state. Orchestration libraries let you choose persistence; AIDB does not make you assemble it.
- an inference engine or model host
- a cloud-only AI platform
- a generic distributed database
- a wrapper that only puts DataFusion + OpenAI + HTTP behind one API
- MCP as a core primitive
- fine-tuning
- a knowledge graph / memory graph
- a huge workflow DSL as the identity of the system
- a secrets product (keys never live in `app.db`; env first, optional store)
- a web UI, auth, or application business logic

If the engine does not eventually choose a cheaper physical plan the developer did not write, we should not be building this. A wrapper is “one API over several backends.” AIDB is only worth building if **the engine owns the physical plan**.

---

## 3. The bar: wrapper vs engine

### Wrapper (do not build this)

```text
User request
  → AIDB
  → Call SQLite
  → Call OpenAI
  → Call API
  → Return result
```

The developer still decides search, then LLM, then retry, then another LLM. Process dies, work dies. Capabilities are functions, not catalog entries the optimizer can read.

`workflow.parallel([...]).then(...)` executed literally, with no rewrite, is persisted LangGraph. That is a wrapper with a database.

### Engine (this is the product)

```text
Developer
  → declarative query / API / SQL
  → IR
  → Optimizer
  → physical plan
  → durable run
  → result
```

Example: Plan A is 10,000 LLM calls ($40). Plan B is filter → embed → TopK → small model → large model ($0.80). A wrapper cannot choose B. An optimizer can.

PostgreSQL is the right analogy, used carefully. You write `WHERE country = 'IN'`. You do not write “use this index.” You still wrote the logical query. You did not write “find me interesting orders” in English.

---

## 4. Two layers (do not confuse them)

| Layer | Input | Output | Analog | When |
| --- | --- | --- | --- | --- |
| Goal planner | Goal + data + capabilities + constraints | Logical IR | NL2SQL / Deep Research | Frontend (`TASK` / `GOAL`) |
| Optimizer | Logical IR + catalog + budgets | Physical plan | Query optimizer | The product |
| Run engine | Physical plan | Durable run + results | Executor + WAL | From the first index job |

“The developer didn’t write the plan” means they didn’t write the *how*. They still wrote the *what*.

A goal language (`TASK investigate_incident`, “find the best 20 candidates”) is a frontend that emits IR. Without an optimizer that can rewrite that IR, it is ChatGPT emitting a DAG. The goal language is not a second engine.

---

## 5. Locked decisions

| # | Decision | Why |
| --- | --- | --- |
| Q1 | **Both — one file, optimizer inside** | `AI.open()` is the product; the optimizer is an engine component |
| Q2 | **Developer declares a query / IR** | Execution stays declarative; the optimizer owns the physical plan |
| Q3 | **First demo: open file, insert docs, search works** | Prove the database before proving optimization |
| Q4 | **TypeScript `AI.open('./app.db')`** | Best developer-facing entry; SQL is the engine |
| Q5 | **Rust core + thin TypeScript / Python bindings** | Runtime in Rust; faces, not a rewrite |
| Q6 | **SQLite now, DataFusion later** | Do not build a relational engine. DataFusion only if a profile says SQLite is the bottleneck |
| Q7 | **`sqlite-vec` in the file** | Vector search is another index, not another product |
| Q8 | **One writer + many readers** | Natural SQLite / WAL. No server required; optional HTTP face over the same file |
| Q9 | **Async re-embed** | Writes must not block on chunk/embed/index |
| Q10 | **Both SQL `AI_GENERATE` and `db.run()`** | Same execution engine underneath; two faces |
| Q11 | **API keys from the environment first** | Catalog stores a key *name*. Optional keychain / `file:` store. Never persist secrets in `app.db` |
| Q12 | **Workflow `then` / `parallel` / `branch` / `loop`** | Compiles to IR; user-facing shape for AI work |
| Q13 | **Checkpoint after each operator** | Genuinely resumable execution |
| Q14 | **Agents are composition, not a table** | `aidb_agent` is a run over models, tools, memory. No `agents` table |

Q2 and Q12 fit because the workflow DSL is a *logical* frontend. It compiles to IR. The optimizer may rewrite it. The developer does not write the physical plan.

---

## 6. Architecture

```text
                         APPLICATION
                              │
                              ▼
                    SQL  (engine)
                    TypeScript / Python  AI.open("./app.db")
                              │
                              ▼
                         AIDB IR          ← we own
                              │
                    ┌─────────┴─────────┐
                    ▼                   ▼
               Optimizer            Run state     ← we own
                    │
                    ▼
              Physical plan
                    │
         ┌──────────┼──────────┐
         ▼          ▼          ▼
      SQLite    AI runtime   Tool runtime
      + vec     (thin)       (catalog + MCP adapter)
         │          │          │
         └──────────┼──────────┘
                    ▼
                 app.db
```

**AIDB decides what and when. Backends decide how.**

```text
                 app.db
                   │
       ┌───────────┴───────────┐
       │                       │
   SQLite / data          AI engine
                               │
                    ┌──────────┼──────────┐
                    │          │          │
                  RAG       Execution  Optimizer
                  Models      Runs
                  Vectors     Workflows
```

Replaceable infrastructure (not the product):

- SQLite today, DataFusion later for heavy analytics — only if profiled
- `sqlite-vec` today, another index later if needed
- OpenAI / Anthropic / Gemini / Ollama / vLLM as adapters
- MCP as a capability adapter, not the internal model

The AIDB execution model is the product. The file is how you hold it.

---

## 7. What lives in the database

**In `app.db`**

- Application tables / JSON (normal SQLite)
- Documents, chunks, metadata
- Embeddings and the vector index (named spaces)
- FTS index (hybrid SEARCH)
- Model catalog (key *names*, no secrets)
- Capability catalog
- Policy (`aidb_meta`)
- Runs, events, checkpoints, artifacts
- Indexing state
- Eval examples; `experiment_results` is a view over runs
- Views: `memory` (documents), `sessions` / `session_turns` (runs)

**Not in `app.db`**

- LLM weights / GPU inference
- API keys (environment first; optional store outside the file)
- An `agents` table, a conversations/messages table, a second store
- Large source artifacts (original PDF stays outside; we store extracted text + `source_uri`)
- Cloud infrastructure, UI, auth, business logic
- External APIs themselves (we record calls; we do not host them)

---

## 8. Developer experience

### Face

```ts
import { AI } from "aidb";

const db = await AI.open("./app.db", {
  embedding: {
    provider: "openai",
    model: "text-embedding-3-small",
    dimensions: 1536,
  },
});

const doc = await db.documents.insert({
  title: "Refunds",
  content: "Refunds are issued within 14 days…",
  metadata: { team: "support" },
});
// returns immediately; index_status = pending

const hits = await db.search("How do refunds work?", { limit: 5 });
// only ready documents

const run = await db.runs.get(doc.index_run_id);
```

Rust IR, CLI, and compiler flags exist underneath. Bindings are faces over the same SQL.

### SQL (same engine)

```sql
SELECT * FROM documents;

SELECT *
FROM documents
SEARCH 'How do refunds work?'
LIMIT 5;

SELECT AI_GENERATE('Summarize this', content)
FROM documents;
```

`SEARCH`, `CREATE MODEL`, and `AI_GENERATE` lower to IR and the same run engine as the TypeScript API. Do not grow a second AI runtime inside SQL UDFs.

### Declarative workload (logical, not physical)

```text
SEARCH documents
→ RERANK
→ GENERATE
→ SAVE
```

becomes IR. The optimizer may decide: filter first, batch, cache, cheaper model, skip generation, run branches concurrently.

### Workflow (compiles to IR)

```sql
SELECT aidb_workflow('{"then":[{"search":{"query":"refunds","k":5}},{"generate":{"prompt":"Summarize this"}}]}');
```

`then` / `parallel` / `branch` / `loop` are logical. Internally:

```text
Workflow spec → Execution IR → Optimizer → Execution plan → Runtime
```

Persisted as `runs` + `checkpoints`, not a second graph store.

### Goal language (frontend, same IR)

```text
TASK investigate_incident
DATA logs, deployments, github
CONSTRAINTS read_only, budget $1, timeout 5m
GOAL identify_root_cause
```

That emits IR. The optimizer may rewrite it. Persisted as a workflow run, not a goals table.

---

## 9. Core components

| Component | Build? | Role |
| --- | --- | --- |
| **IR + contracts** | Yes — core | Typed dataflow DAG. The execution model. |
| **Planner / optimizer** | Yes — core | Equivalence, approximation, physical rewrites. Chooses Plan B. |
| **Run / state engine** | Yes — core | Run ID, node status, checkpoint, crash-resume. |
| **Capability catalog** | Thin | Models, tools, databases: inputs, outputs, cost, side effects, permissions. The optimizer reads this. |
| **Policy** | In the file | `aidb_set_policy`: allow / deny, budgets, HITL overlay. Optimizer and tool runtime read the same object. |
| **Execution bind** | Fold into physical plan | Router is not a product. |
| **AI runtime** | Thin | Embed, LLM, classify. Async, batch, cancel. Structured generate takes a JSON schema. |
| **Data runtime** | SQLite now | Tables, JSON, FTS, transactions. DataFusion later, only if profiled. |
| **Tool runtime** | Catalog + adapters | HTTP, builtin tools, live MCP stdio. MCP is an adapter. |
| **Persistence** | SQLite | The file. |
| **TypeScript / Python SDK** | Thin bindings | `AI.open`, documents, search, runs. Faces, not a second engine. |
| **SQL dialect** | Same engine | `SEARCH`, `AI_GENERATE`, `CREATE MODEL` — lowers to IR. |

Protect three pieces: **IR, optimizer, durable run**. Everything else is a backend or a frontend that emits IR.

---

## 10. IR

The IR is a typed dataflow DAG. Each node is an operator plus a contract plus a schema. Physical planning binds a backend. It does not change logical meaning.

### Logical operators

`Scan`, `Filter`, `Join`, `Aggregate`, `Embed`, `Similarity`, `TopK`, `Llm`, `Tool`, `Then`, `Parallel`, `Branch`, `Loop`, `Decide`

Documents/search are the first concrete instance; generate is the same DAG with an `Llm` node:

```text
Scan(documents) → Filter(index_status = ready) → Embed(query)
  → Similarity → TopK → Llm / Generate
```

### Not IR nodes

| Thing | Where it lives |
| --- | --- |
| Retry | Run policy on a node |
| Checkpoint | Run engine, after node success |
| Wait / Approval | Run status (`suspended`, `awaiting_approval`) |
| SQL text / NL goal | Parsed or compiled *into* IR |
| Agent | Composition: a run (`aidb_agent`) over model + instructions + tools + memory + loop. Not a table. |
| MCP | Adapter into the capability catalog |
| Session | `runs.session_id` plus views. Not a sessions table. |
| Streamed tokens | `run_events` (`kind = token`) on the generate run |

If Retry / Checkpoint / Wait / Approval are IR nodes, the optimizer has to treat a crash boundary as data. Do not do that.

### Logical node shape

```text
LogicalNode
  op            Scan | Filter | Join | Embed | Similarity | TopK | Llm | Tool | …
  schema_in
  schema_out    Arrow-shaped, named columns
  contract      property bag the optimizer reads
  hints         retry class, checkpoint, backend preference
```

### Physical plan

Same DAG, each node bound to a backend and a physical algorithm.

```text
Filter      → SQLite (later DataFusion, only if profiled)
Embed       → AI runtime
Llm         → AI runtime
HTTP GET    → Tool runtime
Approval    → control plane (run state), not a data operator
```

---

## 11. Operator contracts

This metadata is the foundation of the optimizer. Traditional databases have cardinality and cost. AIDB needs a second catalog: what the operator is allowed to do.

| Field | Values | Optimizer uses it for |
| --- | --- | --- |
| `determinism` | Strict \| Approximate \| None | Cache keys, replay, equivalence |
| `side_effect` | None \| Reversible \| Irreversible | Reorder, retry, approval |
| `tuple_independent` | bool | Batch, parallel, per-row cache |
| `listwise` | bool | If true, TopK-before-op is not equivalence |
| `retry` | Safe \| Conditional \| Forbidden | Crash resume, HTTP 429, tools |
| `cache` | Always \| Keyed \| Never | Skip duplicate embed/LLM calls |
| `backend` | Data \| Ai \| Tool \| Control | Physical binding |

### Defaults

| Op | Det | Effect | Indep | Listwise | Retry | Cache |
| --- | --- | --- | --- | --- | --- | --- |
| Scan / Filter / TopK | Strict | None | yes | no | Safe | Always |
| Join / Aggregate | Strict | None | no | no | Safe | Always |
| Embed / Similarity | Approx | None | yes | no | Safe | Keyed |
| Llm (score / classify) | None | None | yes | no | Safe | Keyed |
| Llm (listwise judge) | None | None | no | yes | Safe | Keyed |
| Tool GET | None | None | yes | no | Safe | Keyed |
| Tool POST / email | None | Irreversible | yes | no | Forbidden | Never |

**`listwise` is the trap.** “Pick the best 10 of these 100” cannot be replaced by embed-TopK as an *equivalent* plan. That is an approximation rewrite and must be validated on a sample.

`FILTER → LLM` is not `LLM → FILTER`. Only push filters that do not depend on the LLM’s output columns. Relational algebra does not apply blindly.

---

## 12. Optimizer: three rewrite classes

Do not invent `cost = compute + llm + token + risk` with “Plan A = 95% quality.” Sema (VLDB 2026) and Palimpzest / Abacus already showed static quality numbers fail for LLM operators. Measure dollars and latency under a budget. Check quality against a gold plan on a sample. Miss the floor → widen `k` or fall back.

| Class | Meaning | Examples |
| --- | --- | --- |
| **Equivalence** | Same outputs, cheaper | `PushFilterBeforeExpensive` when the predicate uses only child schema, the expensive op is tuple-independent, no side effect |
| **Approximation** | Different outputs, bounded quality | `CascadeEmbedTopKThenJudge` when the judge is a per-tuple score; sample vs gold |
| **Physical** | Same logical op, different how | `BatchTupleIndependentLlm`, `CacheKeyedAiCall` (key = model + prompt + input + temp); hybrid SEARCH (vec, FTS, or blend) |

Illegal:

- Reorder two LLM ops
- Retry past an irreversible side effect
- Treat listwise judge as equivalent to embed TopK
- Push a filter that reads LLM output columns

Optimizer objective: minimize measured USD and latency under a hard budget. Quality is a constraint, not a predicted 0.95. Experiments persist that comparison as runs; `experiment_results` is a view.

The research question, made falsifiable:

> Can we safely optimize mixed deterministic + probabilistic + side-effecting execution graphs?

---

## 13. Capability catalog and policy

Capabilities are first-class data, not functions.

```text
github.search
github.read
github.write
postgres.query
openai.gpt-x
search.web
send.email
```

Each advertises: inputs, outputs, cost, latency, permissions, side effects, retry, availability.

The optimizer can refuse or rewrite. That catalog is part of the IR story, not a separate platform.

Policy is declarative and in the file (`aidb_set_policy` → `aidb_meta`):

- allow / deny tools, max cost, max runtime, max LLM calls, read_only, require approval
- Goal `CONSTRAINTS` and `AIDB_MAX_*` overlay the same object (tightest wins)
- Irreversible tools still HITL even if policy says allow

Not in the file: secrets, a sidecar policy DB, approval workflows as IR.

Side-effect classes (enforced):

- **Pure** — SELECT, FILTER, EMBED, LLM, CLASSIFY → automatic
- **Reversible** — draft, branch, temp file → checkpoint
- **Irreversible** — charge card, send email, delete, deploy → approval / idempotency / explicit policy

---

## 14. Run and state

DataFusion / a normal wrapper thinks: query → result. AIDB thinks: a durable run that can die and continue.

### Run lifecycle

```text
Pending → Planning → Running → Completed | Failed | Cancelled
Running ⇄ Suspended
Running ⇄ AwaitingApproval
```

Resume is `aidb_resume(id, { approved: true })`. Approval is a run status, not an IR node.

Generate tokens append as `run_events` (`kind = token`) on the same run. The SQL result is still the full text. A reconnect reads the prefix from the file.

`session_id` is a nullable column on `runs`. `SELECT aidb_session('desk')` binds the connection. `sessions` / `session_turns` are views. There is no sessions table and no conversations/messages table.

### Node lifecycle

| State | Meaning | Durable |
| --- | --- | --- |
| Pending | Inputs not ready | yes |
| Scheduled | Backend chosen | yes |
| Running | In flight; output not committed | yes |
| Succeeded | Artifact written | yes |
| Failed | Error recorded; retry policy applies | yes |
| Skipped | Upstream empty | yes |

### Checkpoint

After each completed operator. Not mid-token. Not an IR node.

Crash:

```text
load run
  → replay Succeeded from artifacts / tables
  → reschedule Running if retry = Safe
  → do not auto-retry Irreversible tools
```

First `kill -9` test is **document indexing**, not a multi-agent graph:

```text
chunk → embed → upsert vec
```

Crash during embed: chunks are already in the table; resume missing vec rows.

### One engine

```text
SQL  AI_GENERATE       ─┐
SQL  aidb_agent        ─┤
TS   db.run()          ─┼→ IR → execution engine → runs / events / checkpoints
TS   documents.insert  ─┘   (kind = index_document)
```

---

## 15. Storage (`SCHEMA_VERSION` 9)

Canonical schema: [`schema/v001.sql`](schema/v001.sql). Open applies additive migrations through [`schema/v009.sql`](schema/v009.sql). Do not rewrite `v001.sql`.

### On disk

| Path | Role |
| --- | --- |
| `app.db` | Canonical database |
| `app.db-wal` | WAL |
| `app.db-shm` | Shared memory |

Backup: copy after checkpoint, or `VACUUM INTO`. No sidecar directory. Small artifacts stay as JSON in SQLite.

### Pragmas at open (engine, not the SQL file)

| Pragma | Value | Why |
| --- | --- | --- |
| `journal_mode` | WAL | Readers do not block the writer |
| `foreign_keys` | ON | Document delete cascades chunks |
| `busy_timeout` | 5000 | Retry instead of `SQLITE_BUSY` |
| `synchronous` | NORMAL | Safe with WAL |
| `temp_store` | MEMORY | Sorts / FTS scratch off disk |

### Connections

- **One write connection** in Rust, behind a mutex. All TS inserts, status updates, and vec upserts go through it.
- **Read pool** of read-only connections for SEARCH and SELECT. They see a WAL snapshot. Search never takes the write lock.

### Tables

| Table | Purpose |
| --- | --- |
| `aidb_meta` | `schema_version`, embedding space, policy |
| `documents` | Source text + `index_status` |
| `chunks` | Integer PK (aligns with vec0) |
| `chunks_fts` | FTS5; hybrid SEARCH blends this with vec |
| `vec_chunks` | `sqlite-vec` KNN; created once dimensions are known |
| `embedding_spaces` | Named spaces; default space stays `aidb_meta` + `vec_chunks` |
| `models` | Catalog only. `key_name`, never the secret |
| `capabilities` | Tool catalog. MCP writes rows here |
| `eval_examples` | Labeled gold for experiments |
| `runs` | Durable execution. Kinds include `index_document`, `search`, `generate`, `workflow`, `agent`, `tool`, `experiment` |
| `run_events` | Append-only log, including generate tokens |
| `checkpoints` | Operator resume |

**Views (not tables):** `memory` over documents; `sessions` / `session_turns` over runs; `experiment_results` over experiment child runs.

**Not in the file:** an `agents` table, conversations/messages, secret values, DataFusion catalogs, a second store.

### Documents

`insert` persists the row and enqueues an index run in the **same transaction**. The application does not wait for chunk / embed / index.

| `index_status` | Meaning | In SEARCH? |
| --- | --- | --- |
| `pending` | Written, run not started | no |
| `indexing` | Chunk / embed / vec in progress | no |
| `ready` | Chunks + vec committed | yes |
| `failed` | `index_error` set; content kept | no |

Update with a new `content_hash`: delete old chunks (cascade + FTS triggers + vec delete), enqueue a new index run. Same hash is a no-op. Delete removes document, chunks, FTS, and vec rows in one writer transaction.

`source_uri` points at an external PDF or blob. `content` is extracted text we own.

### Vectors

The default space is declared on first open or first embed and stored in `aidb_meta` + `models`. Named spaces live in `embedding_spaces` with their own vec table. Changing a space’s model or dimensions is a rebuild. Mismatch on `AI.open()` fails closed.

```sql
CREATE VIRTUAL TABLE vec_chunks USING vec0(
  chunk_id INTEGER PRIMARY KEY,
  embedding float[<D>] distance_metric=cosine,
  document_id TEXT
);
```

`document_id` is a short metadata column for filtered KNN. Chunk text stays in `chunks` and is joined after KNN.

`SEARCH` / `aidb_search`: embed the query (`kind = embed_query`), then a physical plan — vec KNN, FTS, or a blend — join ready documents, optional metadata filter and space. Default limit 5. One function.

### Run kinds

| `kind` | When | Checkpoint |
| --- | --- | --- |
| `index_document` | After `documents.insert` | After chunk, after embed |
| `embed_query` | SEARCH | Optional |
| `search` | Optional wrapper around retrieval | no |
| `generate` | `AI_GENERATE` / `aidb_classify` / `db.run` | After the model call |
| `workflow` | Compiled DSL / goal language | After each operator |
| `agent` | `aidb_agent` (parent); children are search / generate / tool | After each step |
| `tool` | Capability invocation | After the call |
| `experiment` | Parent comparison; each child is a named plan | After the plan |

---

## 16. Models

First-class catalog, not hosted inference.

```text
CREATE MODEL gpt
PROVIDER openai
MODEL 'gpt-5.6';
```

Kinds: LLM, embedding, reranker. Adapters: OpenAI, Anthropic, Gemini, Ollama, vLLM, local / custom. No model weights in the file. Classify is the same `Llm` path (`aidb_classify` writes a `generate` run). Structured generate / classify take an optional JSON schema as the third argument; mismatch fails the run.

Keys:

```bash
OPENAI_API_KEY=...
ANTHROPIC_API_KEY=...
```

Env first. Optional `AIDB_SECRET_STORE=keychain` or `file:/path`. The catalog stores a key *name*, never the secret. Reopen without the store is a missing-key error, not a corrupt file.

---

## 17. Retrieval, RAG, memory

### Retrieval

The developer should not wire embed → vector DB → filter → rank → fetch.

Internally:

```text
Query → Embed → Vector search / FTS / hybrid → Metadata filter → Rank → Results
```

Retrieval: semantic / vector, FTS, hybrid (physical plan of one `aidb_search`), metadata filtering, named embedding spaces.

Do not build knowledge graphs or graph databases.

### RAG

Composition of primitives, not a separate framework: documents → embeddings → retrieval → context → model → answer. `aidb_generate` / `AI_GENERATE` over `aidb_search` returns citations:

```json
{
  "answer": "...",
  "sources": [
    { "document_id": "doc_123", "chunk_id": "chunk_8", "score": 0.91 }
  ]
}
```

Plain generate stays a string. No citations table.

### Memory

Not a magical subsystem. Documents with `metadata.kind = 'memory'`. The `memory` view is that slice. Search is `aidb_search` / `aidb_memory_search`.

```ts
await db.memory.insert({ userId: "123", content: "Prefers concise technical explanations." });
await db.memory.search({ userId: "123", query: "How should I explain this?" });
```

Shared agent memory is the same tables, not a hidden context object. There is no conversations/messages table.

---

## 18. Workflows and agents

### Workflows

User-facing: `aidb_workflow` with `then`, `parallel`, `branch`, `loop`. Internally compiled to IR. Persisted as `runs` + `checkpoints`, not a second graph store.

Human-in-the-loop is a run state (`awaiting_approval`) plus `aidb_resume(id, { approved: true })`. Not an IR node.

### Agents

An agent is **not** a permanent database object and **not** the fundamental abstraction.

```text
Agent = model + instructions + tools + memory + execution loop
```

`SELECT aidb_agent('{…}')` opens a parent run. Children are search / generate / tool runs under the same policy, HITL, and file. A decide agent (`decide: true`) lets the model choose the next catalog op. Dynamic agents are child runs. Multi-agent systems are composition of execution primitives.

Persist the **execution**, not an agent definition. No `agents` table.

---

## 19. Technical stack

```text
TypeScript / Python SDK
      │
 thin binding (napi / PyO3)
      │
    Rust
      │
 ┌────┼────────────┐
 ▼    ▼            ▼
SQLite  AI runtime  Optimizer
+ vec   adapters    + IR + run
```

Rust owns: storage integration, execution, concurrency, embeddings orchestration, retrieval, workflow runtime, optimizer, persistence.

TypeScript and Python own: developer API, ergonomic types, application integration. They wrap the same file. They do not spawn the CLI.

Optional: `aidb serve` (HTTP) and Studio (inspect) over the same file.

### `AI.open` contract

| Field | Required? | Notes |
| --- | --- | --- |
| `path` | yes | `./app.db` |
| `embedding.provider` | on first embed | openai / ollama / … |
| `embedding.model` | on first embed | stored in `aidb_meta` + `models` |
| `embedding.dimensions` | on first embed | locks `vec0` |
| keys | never in options | environment first; optional store |

---

## 20. Crate / package map

| Piece | Job |
| --- | --- |
| `schema/v001.sql` … `v009.sql` | Canonical SQLite schema (`SCHEMA_VERSION` 9) |
| `aidb-ir` | Op enum, schema, `OperatorContract`, logical / physical plan |
| `aidb-opt` | Rewrite trait; three rewrite classes; gold-sample check |
| `aidb-run` | Run state machine, SQLite, artifacts, resume, `session_id` |
| `aidb-sql` | `aidb_search`, generate, classify, dialect, goal language |
| `aidb-tool` | Capability catalog, policy, MCP stdio |
| `aidb` | Public Rust crate: open, execute, query |
| `aidb` (TS / Python) | Faces: `AI.open`, documents, search, runs |
| `aidb-cli` | `aidb sql`, status, resume, explain plan, optional `aidb serve` |
| `aidb-server` / Studio | Optional inspect faces over the same file |

Bindings do not get their own storage layer.

---

## 21. Order of the thesis

Do not start with agents. Do not start with the optimizer. Do not start inside DataFusion.

The engine follows that order: file and documents first, then generate and durable runs, then IR, workflow, optimizer, then agents as runs. Bindings are faces. Everything else in this document lives in the same file.

Remaining numbered product work: DataFusion, last, only if a profile says SQLite is the bottleneck. See [`PHASES.md`](PHASES.md).

---

## 22. Success bars

### Substrate

```text
AI.open()
  → persistent DB
  → insert documents
  → async embed + sqlite-vec
  → SEARCH
  → results
```

If this does not feel excellent, the optimizer does not matter.

### Optimizer

Same workload, dramatically fewer / cheaper model operations, quality held on a labeled slice, plan a human can read. Queryable as `experiment_results`.

Concrete form:

```text
10,000 candidates
  → cheap filters
  → embed
  → TopK
  → LLM judge
  → 20
```

vs naive 10,000 LLM calls. Gold plan on a sample. Run survives `kill -9` and resumes.

### Long-term bar

> We created an execution model where the engine understands data, reasoning, tools, and constraints, and optimizes how the whole workload runs.

If that sentence is not true, stop.

---

## 23. Related work (know what we are not)

| System | Owns | AIDB overlap |
| --- | --- | --- |
| **Sema** (DuckDB, VLDB 2026) | Semantic SQL operators, AQE, prompt batching, NL→SQL pushdown | Proof that AI operators belong in the plan, not UDFs. They deferred model routing, fault tolerance, governance. Static cost models fail. |
| **LOTUS** | DataFrame semantic ops, cheap proxies with quality bounds | Cascade / proxy-then-gold is an early optimizer win |
| **Palimpzest / Abacus** | Cost-based physical choices via sampling | Sample-based cost, not a spreadsheet 95% |
| **DocETL** | Agentic pipeline rewrites for document accuracy | Do not compete on document rewriting |
| **Russo & Kraska, CIDR 2026** | Deep Research agents that emit optimized semantic programs | The “find the root cause” goal frontend |
| **Trellis / Aster** | Durable agent state, graphs, blackboards | State layers. Use later if SQLite runs out. Not the thesis. |

Differentiation, kept narrow: semantic engines optimize bulk LLM-over-data; agent runtimes orchestrate tools and state. AIDB is interesting if **one optimizer sees both**, with operator contracts as the shared language, inside an embedded database file.

AIDB has a database-style optimizer whose job is to transform AI execution plans under cost and latency constraints. That is the claim to prove (experiments in the file), not “LangGraph has no optimizer.”

Public claim: AIDB is designed to make an additional orchestration stack unnecessary for the core AI runtime. Developers may still use other libraries. Internally: if the *runtime* needs it, it lives in `app.db` or it is a provider — it is not a second orchestration engine.

---

## 24. Out of scope by horizon

**In the file**

The engine this document describes: documents, hybrid SEARCH, generate / classify, model catalog, durable runs, IR, optimizer, workflows, agents as runs, HITL, tools + MCP adapter, memory as documents, SQL dialect, goal language, RAG citations, metadata filters, policy, embedding spaces, experiments, sessions as views, streaming tokens as `run_events`. Optional `aidb serve` / Studio inspect the same file. Env-then-store secrets; never keys in the file.

**Later, only if profiled**

DataFusion for heavy analytics when SQLite is the bottleneck.

**Not the product**

Own storage engine, own vector database, own LLM, graph database, cloud control plane, multi-agent framework as identity, an `agents` table, a conversations/messages table, a second store.

---

## 25. Product boundary

**AIDB provides:** data (tables, JSON, documents, memory), retrieval (embeddings, vector search, FTS, hybrid, RAG citations), AI (models, generation, classify, structured output), execution (runs, workflows, tools, state, events, sessions as views).

**The application provides:** UI, authentication, business logic, application APIs, domain-specific tools.

**Providers provide:** LLMs, embedding models, rerankers.

---

## 26. Mental model

Do not make “Agent” the fundamental abstraction. Make **persistent AI execution** the fundamental abstraction.

Agents, workflows, RAG, memory, tools, evaluation, and observability compose from:

```text
data + retrieval + model calls + durable runs + an optimizer that may rewrite the plan
```

That is closer to “DuckDB for AI applications” than another AI framework — and it is only a real database if the engine, not the developer, eventually owns the physical plan.
