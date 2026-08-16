# AIDB

An embedded database for AI applications. **One file:** data, retrieval, and durable runs.

You open `app.db`, run SQL, and get rows. TypeScript, Python, the CLI, HTTP, and Studio are faces over that file. They are not a second engine.

**Status:** v0 (`0.0.0`). Phases 0–26, 28–35 are done. Phase 27 (DataFusion) is last, only if a profile says SQLite is the bottleneck.

| | |
| --- | --- |
| Product | [`DESIGN.md`](DESIGN.md) |
| What shipped | [`PHASES.md`](PHASES.md) |
| Guides | [`docs/`](docs/README.md) |
| License | [MIT](LICENSE) |

## Why

LangChain and LangGraph help you *assemble* an AI application. AIDB is the
persistent runtime that application can run on: documents, embeddings, model
calls, tools, and crash-resume as rows in SQLite.

Copy the file and you copied the application's state, including the audit trail.
There is no vector service, trace backend, or checkpoint store to keep in sync.

**Not the product:** an `agents` table, a conversations table, a second store, a
chat UI, or hosting models.

## Install

Until packages are on the registries, install from this repository (needs a Rust
toolchain for the native addon):

```bash
git clone https://github.com/yogthesharma/aidb.git
cd aidb
cargo build --workspace
npm i ./bindings/typescript          # TypeScript face
pip install ./bindings/python        # Python face
cargo install --path crates/aidb-cli # `aidb` CLI
```

That installs the native addon (`aidb.node` / `aidb_native`) and the CLI. You do
not copy a `.dylib` or `.so` by hand.

Prebuilt napi addons and Python wheels (when published): **macOS arm64**,
**Linux x64 (gnu)**, **Windows x64**. Other hosts: build from this repo.

## Use

```ts
import { AI } from "aidb";

const db = await AI.open("./app.db");
await db.query(
  "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')"
);
const hits = await db.search("how do refunds work?", { limit: 3 });
```

```python
from aidb import AI

db = AI.open("./app.db")
db.query(
    "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')"
)
hits = db.search("how do refunds work?", limit=3)
```

```bash
aidb sql ./app.db "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
aidb sql ./app.db "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')"
aidb sql ./app.db "SELECT id, kind, status FROM runs ORDER BY created_at_ms DESC LIMIT 5"
```

```sql
SELECT aidb_search('how do refunds work?', 3);
SELECT aidb_generate('Summarize this', content) FROM documents;
SELECT aidb_classify('positive or negative', 'This refund was a negative surprise.');
SELECT aidb_last_run_id();
SELECT aidb_session('desk');
SELECT aidb_agent('{"instructions":"Answer from documents. End with DONE.","goal":"How do refunds work?","tools":["search","generate"],"max_steps":4}');
SELECT id, json_extract(output_json, '$.message') FROM runs WHERE status = 'awaiting_approval';
```

Keys stay in the environment (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, …). They are
never stored in the file. Without keys, generate uses a fake model so tests stay
offline.

More SQL: [`docs/sql.md`](docs/sql.md). First file: [`docs/getting-started.md`](docs/getting-started.md).

## Inspect

```bash
aidb serve ./app.db          # HTTP over the same file
cd studio && npm run dev     # inspect face at http://127.0.0.1:5173
```

See [`docs/http.md`](docs/http.md) and [`studio/README.md`](studio/README.md).

## A real app

[`examples/stock`](examples/stock/README.md) is an equity research desk with no AI
framework behind it. Filings, embeddings, cited answers, agent steps, approvals,
and the desk's own tables live in one file.

```bash
node examples/stock/stock.mjs demo --db /tmp/desk.db
```

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

Details: [`CONTRIBUTING.md`](CONTRIBUTING.md). Security: [`SECURITY.md`](SECURITY.md).

## License

[MIT](LICENSE) © 2026 Yog Sharma and AIDB contributors.
