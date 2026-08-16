# Where AIDB is now

**v0 (`0.0.0`).** The engine is done enough to build real apps on one SQLite file. Phases 0–26 and 28–35 shipped. Phase 27 (DataFusion) is not started — only if a profile says SQLite is the bottleneck.

Rust + SQL is the engine. TypeScript and Python are faces over that file, not a second runtime.

## What works

Open a file, run SQL, get rows. Copy the file and you copied documents, embeddings, runs, approvals, and spend.

| Surface | What it is |
| --- | --- |
| Documents + hybrid search | `aidb_insert_document`, `aidb_search` (FTS + vec, metadata filter, named spaces) |
| Generate / classify / schema | `aidb_generate`, `aidb_classify`, JSON Schema on generate (invalid output fails the run) |
| Memory | `aidb_memory_insert` / `_search` — documents with `kind=memory`, not a chat store |
| Sessions | `aidb_session` + `session_turns` view over `runs` |
| Agents / workflows / HITL | `aidb_agent` (including `"decide":true`), `aidb_workflow`, `aidb_resume` |
| Policy + tools | `aidb_set_policy`, capability catalog, `aidb_mcp_register` / `aidb_mcp_connect` |
| Experiments | `eval_examples` + `aidb_experiment` → `experiment_results` |
| Streaming | generate tokens as `run_events` (`kind='token'`); reconnect still has the prefix |
| Models | `CREATE MODEL` — fake (offline), OpenAI, Anthropic, Kimi/Moonshot for LLM; embeddings are fake, OpenAI, local (BGE/Nomic/E5), or in-process custom |
| Faces | CLI (`aidb sql` / `aidb serve`), TypeScript `AI.open`, Python `AI.open`, Studio inspect UI |

Shipped apps in this repo: [`examples/stock`](examples/stock/README.md) (CLI desk), [`examples/support`](examples/support/README.md) (support UI), [`examples/chat`](examples/chat/README.md) (Ada chat).

## LangChain / LangGraph / LangSmith — can it replace them?

**Not a drop-in clone, and that is intentional.** Those libraries help you *assemble* an application (chains, graphs, traces). AIDB is the **file the application runs on**: data, retrieval, model calls, tools, and crash-resume as rows. Persistence is the product, not “better agents.”

| They do | AIDB as of now |
| --- | --- |
| **LangChain** — prompt templates, LCEL, hundreds of integrations, document loaders, output parsers | **Partial.** Generate / classify / schema / search / memory are native SQL. No loader zoo, no LCEL, no retriever interface. You ingest and prompt yourself. |
| **LangGraph** — arbitrary Python/TS graphs, cycles, subgraphs, checkpoint backends you pick | **Partial, different shape.** Orchestration is IR in the file (`then` / `parallel` / `branch` / `loop` workflows, recipe agents, `"decide":true` agents). Not a graph SDK. `max_steps` + policy bound the loop. Kill the process; the run row is still there. |
| **LangSmith** — hosted traces, datasets, eval UI, team debugging | **Partial, in-file.** Every generate/search/tool/agent is a `runs` row; tokens are `run_events`; evals are `aidb_experiment` → `experiment_results`. Studio inspects the same file. No hosted SaaS, no team cloud, no LangSmith-compatible exporter. |

**Replace the stack for a one-file app?** Yes, for the core runtime — if you are willing to write SQL (or a thin backend that does), own ingest/UI, and stay inside catalog tools + HITL. The stock desk and Harbor/Ada examples do that with no LangChain/Graph/Smith.

**Replace them in a large existing LangGraph codebase?** No. There is no import compatibility, no graph compile, no LangSmith project. You would rewrite orchestration as `aidb_workflow` / `aidb_agent` and traces as `SELECT`s.

You can still call LangChain from your app process. Internally AIDB will not grow a second orchestration engine: if the runtime needs it, it lives in `app.db` or it is a provider.

## Orchestration as of now

There is no `agents` table. An agent or workflow **is a parent run**. Children are search / generate / tool / decide rows. Checkpoints sit on that run. `EXPLAIN` prints the plan.

```text
aidb_workflow JSON  ──►  IR (then / parallel / branch / loop)
aidb_agent JSON     ──►  recipe (tools in order until DONE)
                        or decide loop (model picks op + args)
                              │
                              ▼
                         runs + checkpoints
                         policy / HITL / budget
```

| Kind | How you write it | What happens |
| --- | --- | --- |
| **Static workflow** | `SELECT aidb_workflow('{"then":[{"search":…},{"approve":…},{"generate":…}]}')` | Declared graph. Compiles to IR. Optimizer may rewrite. Checkpoint after each operator. |
| **Recipe agent** | `aidb_agent` with `tools` (default) | Walks the tool list until the model says DONE or `max_steps`. |
| **Decide agent** | `aidb_agent` with `"decide":true` | Each step is a schema-valid choice: search / generate / catalog tool / stop, with JSON args (filters, `send.email` to, …). |
| **HITL** | irreversible tools + `require_approval`, or an `approve` node | Run parks `awaiting_approval`. `aidb_resume(id, '{"approved":true}')` continues with **stored** args. Process crash does not lose the draft. |
| **Policy** | `aidb_set_policy` | Allow-list, `max_usd` / `max_llm_calls`, overlay with `AIDB_MAX_*`. Tightest wins. |
| **Session** | `aidb_session('desk:nvda')` | Following runs get `session_id`. `session_turns` is a view. |
| **Inspect** | `runs`, `run_events`, Studio, `aidb serve` `/ws` | Spend, tokens, errors, token prefix on reconnect. |

What orchestration is **not**, yet: unconstrained graphs, subgraphs as a library, human-in-the-loop as a LangGraph interrupt API, distributed workers, a visual graph editor, or “the model writes arbitrary Python.” Bound loops in one process, one writer, one file.

## What does not work / will not be the product

- **Not on npm or PyPI yet.** Install from this repo.
- **No DataFusion.** SQLite is the store until a profile says otherwise.
- **No `agents` table, no conversations table.** An agent is a parent run. A thread is `session_id` on `runs`.
- **No second store** (no vector SaaS, no LangSmith-style trace backend, no extra checkpoint DB).
- **Does not host models.** Keys stay in the environment; they are never written into the file.
- **Kimi is LLM-only.** Embeddings are not a Moonshot provider (use fake, OpenAI, or local).
- **Not NL2SQL as the product.** SQL is the surface; you write it (or your app does).
- **MCP stdio** needs a local binary; the chat example does not spawn one.

## Apps you can build

The app owns UI, auth, and domain tables. AIDB owns retrieval, model calls, tools, and crash-resume in the same file.

Good fits: cited knowledge / support bots, research desks, classifiers, schema extraction, approval-gated email/ops agents, decide-loop analysts, per-user memory, threaded research chat, eval bakeoffs, multi-space (legal vs product) search, policy-bounded copilots.

Pattern: start from documents and `runs`, not from a ChatGPT clone schema. A chat UI is fine if the transcript is `session_turns` (see `examples/chat`).

## Rust, JavaScript, Python

| | Status |
| --- | --- |
| Engine | Rust. Not ported away from Rust. |
| TypeScript face | **Done.** In-process napi addon (`AI.open`). Same SQL, same file. |
| Python face | **Done.** In-process PyO3 module (`AI.open`). Same SQL, same file. |
| Packages on registries | **Not yet.** `npm i aidb` / `pip install aidb` from a registry is not how you get it today. |

**Do you need a Rust toolchain?** Yes, to *build from this repository* (`cargo build --workspace`), because the Node addon and Python module are native. After that, running an app is Node or Python plus the built addon — you do not rewrite the engine in JS/Python.

When prebuilt napi addons and wheels are published: macOS arm64, Linux x64 (gnu), Windows x64, and those hosts will not need Rust just to install. Other hosts still build from this repo.

```bash
git clone https://github.com/yogthesharma/aidb.git
cd aidb
cargo build --workspace
pnpm example:chat      # Ada UI, after a build
```

More: [`README.md`](README.md), [`docs/sql.md`](docs/sql.md), [`docs/apps.md`](docs/apps.md), [`PHASES.md`](PHASES.md).
