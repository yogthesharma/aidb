# Getting started

AIDB is an embedded database. You open a file, run SQL, and get rows. TypeScript,
Python, the CLI, HTTP, and Studio are faces over that file.

## From this repository

You need a Rust toolchain. Node and Python are optional until you use those faces.

```bash
git clone https://github.com/yogthesharma/aidb.git
cd aidb
cargo build --workspace
```

Open a file and run SQL:

```bash
cargo run -p aidb-cli -- sql ./app.db "SELECT value FROM aidb_meta WHERE key = 'schema_version'"
```

Insert a document, search, generate (offline fake provider by default):

```bash
cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_insert_document('Refunds', 'Refunds are issued within 14 days of purchase.', '{}')"
cargo run -p aidb-cli -- sql ./app.db "SELECT document_id, content FROM aidb_search('how do refunds work?', 3)"
cargo run -p aidb-cli -- sql ./app.db "SELECT aidb_generate('Summarize this', 'Refunds are issued within 14 days of purchase.')"
cargo run -p aidb-cli -- sql ./app.db "SELECT id, kind, status FROM runs ORDER BY created_at_ms DESC LIMIT 5"
```

The CLI drains the indexer after an insert. `inserted` is not `searchable` until
`documents.index_status = 'ready'`.

## TypeScript and Python faces

Stage the native addon from this repo, then open the **same** file:

```bash
npm i ./bindings/typescript
# or: pip install ./bindings/python
```

```ts
import { AI } from "aidb";
const db = await AI.open("./app.db");
await db.query("SELECT aidb_search('how do refunds work?', 3)");
```

Keys stay in the environment (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`). They are
never stored in the file. Without keys, generate uses the fake model so tests and
demos stay offline.

## Next

- [SQL surface](sql.md) — generate, classify, agents, sessions, tokens
- [HTTP and Studio](http.md) — inspect the same file in a browser
- [`examples/stock`](../examples/stock/README.md) — an AIDB-only application
- [`PHASES.md`](../PHASES.md) — what each phase proved
