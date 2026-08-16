# Phase 29 log — building an app on AIDB only

Every time the stock desk needed something the engine did not hand it, the question
was the same one from [`PHASES.md`](../../PHASES.md):

> Should this become an AIDB primitive, or does it belong to the application?

No LangChain, no LangGraph, no LangSmith, no vector service, no trace backend. What
follows is what actually happened, in the order it happened.

## Fixed in the engine

**1. A projected retrieval returned the wrong columns.**
`SELECT document_id, content FROM aidb_search(...)` returned all four retrieval
columns and ignored the column list, so a caller reading by position got `chunk_id`
where it asked for `content`. Our own TypeScript memory face did exactly that: the
desk injected the chunk id `8` into a prompt as an analyst preference.

Primitive, and a bug: SQL is the surface, so a column list has to mean something.
`aidb_sql::project_selection` now applies a plain column list to `aidb_search` and
`aidb_memory_search`; `*` and expressions still return the whole row, and an unknown
column is an error instead of four silent columns.
Regression: `a_projected_retrieval_returns_the_columns_the_caller_asked_for`.

**2. An approved agent asked for approval again.**
The digest agent is search → generate → `send.email`. The model says DONE while
drafting, then the email tool runs after it and its output replaced the DONE signal,
so the loop went round again, hit the irreversible tool again, and parked again. One
approval never finished the run, and the desk's approval queue never drained. It was
invisible to the suite because every existing HITL test used a single-tool,
single-step agent.

Primitive, and a bug: only the model can end the loop, and a tool that runs after it
must not erase that. Regression:
`an_approved_digest_finishes_instead_of_asking_again`.

**4. Naming a run that a scalar function created.**
`aidb_classify` returns a label, not the run that produced it. Guessing the newest
`generate` row by time is fine for one writer and wrong under concurrency.
Closed: `SELECT aidb_last_run_id()` is this thread's last insert, not a guess
by timestamp. Same connection/thread model as `aidb_session`. A new thread
starts empty until it inserts.

**10. Parked `output_json` is always JSON.**
A parked agent used to store the plain approval message; a workflow pause was
inconsistent. Closed: `park_run` stores
`{"paused":true,"status":"<status>","message":"<human message>"}`. The SQL `output`
column stays the human text. `json_extract(output_json, '$.message')` is the
message; old files may still hold a plain string.

## Left to the application

**3. Waiting for a document to be searchable.**
Indexing is a background run, so "inserted" is not "searchable". The bindings and
the CLI drain the indexer after an insert, but nothing in SQL says *wait*. The app
polls `documents.index_status` — six lines, no primitive. Revisit only if an app
needs to wait on someone else's write.

**5. The TypeScript face is thinner than SQL.**
No filtered search, no policy, tools, workflows or classification helpers. The desk
uses `db.query` with SQL for those, which is the design: the face stays thin and SQL
is the surface. Not a gap.

**6. Re-ingesting the same filing.**
Inserting identical text twice is two documents; the engine only treats a *repeat
update to the same document id* as a no-op. Running `ingest` twice gave the desk 14
documents and a corpus that cites itself twice.

Application. What counts as "the same filing" is domain knowledge, so the desk keys
its own documents (`metadata.source_id`) and skips what it already has. No primitive
wanted: an engine-level dedupe would have to guess the key.

**7. Domain everything.** Watchlist and signals are ordinary tables the app created
in the same file, and they join to `runs` and `documents` directly. No primitive
wanted here — this is the part that should be boring.

## Closed later (logged here, built as file-shaped primitives)

**8–9. Closed in Phase 33.** A workflow `approve` then irreversible `tool` used
to fail on resume, and a recipe agent could not pass tool arguments or scope
search. `"decide":true` makes each agent step a schema-valid choice with
arguments. A workflow `approve` then `tool` now runs after resume. Recipe agents
are unchanged. Not a graph library.

**Closed in Phase 34.** A session is a thread of runs (`session_id` plus the
`sessions` / `session_turns` views), not a conversations table. The desk never
grew a chat UI; Turn 1 / 2 / 3 is a SELECT when an app wants it.

**Closed in Phase 35.** Tokens append to the generate run as events. A reconnect
still has the prefix. The desk never needed a streaming UI; `SELECT` from
`run_events` is enough when an app wants it.

## What the desk never needed

A conversations table, a second store, an `agents` table, or a trace backend.
The transcript is `runs` (optionally grouped by `session_id`, with generate
tokens on `run_events`), the memory is documents, the budget is a policy row,
and the audit trail is the file. Phase 27 stays last, only if a profile says
SQLite is the bottleneck.
