# Contributing to AIDB

Thank you for wanting to work on this. Product and architecture live in
[`DESIGN.md`](DESIGN.md). Build order and the SQL-first surface live in
[`PHASES.md`](PHASES.md). Please read both before a large change.

The [Code of Conduct](CODE_OF_CONDUCT.md) applies.

## What this project is

AIDB is an **embedded database**. SQL is the surface. Bindings, the CLI, HTTP, and
Studio are faces over the same file. A behaviour change has to stay true to that,
or it is probably the wrong change.

Do **not**:

- Add an `agents` table, a conversations/messages table, or a second store
- Start Phase 27 (DataFusion) unless a profile shows SQLite is the bottleneck
- Make provider SSE, LangGraph, or a chat UI the identity of the product
- Put secrets in the file

A new primitive is allowed only if a real AIDB-only app needed it and it belongs
in the file (not the application, not a provider). That rule is in PHASES.md
after Phase 28.

## Prerequisites

- Rust (stable), for `cargo test --workspace`
- Node.js 20+, for the TypeScript face and Studio
- Python 3.11+, for the Python face
- No provider keys for the default suite (it uses the fake LLM / embedder)

## Build and test

From the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
node bindings/typescript/scripts/stage-native.mjs
python3 bindings/python/scripts/stage_native.py
cargo test --workspace
```

Face suites (point them at the CLI you just built):

```bash
AIDB_CLI_BIN=./target/debug/aidb node bindings/typescript/test.mjs
AIDB_CLI_BIN=./target/debug/aidb python3 bindings/python/test_open.py
```

Stock desk and Studio:

```bash
node examples/stock/stock.mjs demo --db /tmp/desk.db
cd studio && npm ci && npm test && npm run build
```

`cargo test` does **not** rebuild the `aidb` binary the CLI tests spawn. Build
first, or you will debug a stale engine.

Live provider tests are opt-in and cost money:

```bash
AIDB_LIVE_TESTS=1 OPENAI_API_KEY=… cargo test -p aidb --test live_providers
```

## How to change the engine

1. Prefer a SQL demo in `examples/sql/` that a user can paste into `aidb sql`.
2. Add or extend a contract test under `crates/aidb/tests/` (or the crate that
   owns the behaviour).
3. If Studio's pages are SELECTs, keep `studio/src/lib/catalog.mjs` in lockstep
   with `crates/aidb/tests/studio.rs`.
4. Bindings stay thin: they wrap SQL. Do not grow a second API that the engine
   does not have.

Schema changes go in a new `schema/vNNN.sql` and bump `SCHEMA_VERSION`. Do not
rewrite `schema/v001.sql`.

## Pull requests

- Keep the diff reviewable. One concern per PR when you can.
- `cargo fmt` and `clippy -D warnings` must pass; CI gates both.
- Update PHASES.md / DESIGN.md only when the product contract changed.
- Do not commit `app.db`, native `.node` / `.so` addons, or `node_modules`.

## License

Contributions are under the MIT license in [`LICENSE`](LICENSE).
