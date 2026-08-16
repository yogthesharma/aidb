# AIDB Studio

Inspect face over `aidb serve`. Same file. Not a second engine. Not the chat product.

The browser talks to `POST /sql`, `GET /health`, and `GET /ws`. Pages are `SELECT`s: file, documents, search, runs, experiments, models. Approve is `SELECT aidb_resume(...)`.

## Run

Terminal 1 — engine, one writer, loopback by default:

```bash
cargo run -p aidb-cli -- serve ./app.db
```

Terminal 2 — UI:

```bash
cd studio
npm install
npm run dev
```

Vite listens on `http://127.0.0.1:5173` and proxies `/sql`, `/health`, and `/ws` to `http://127.0.0.1:8080`. Override with `AIDB_SERVE_URL`.

```bash
AIDB_SERVE_URL=http://127.0.0.1:9090 npm run dev
```

## Bearer

When serve is started with `AIDB_BEARER` / `AIDB_TOKEN`, Studio must send the same token. Three ways, all the same gate:

- Key icon in the header (stored in this browser only)
- `http://127.0.0.1:5173/file?token=...` (copied into localStorage, then stripped from the URL)
- `AIDB_BEARER=... npm run dev` (Vite injects `Authorization` on the proxy)

Studio sends `Authorization: Bearer` on `/sql` and `/health`, and `?token=` on `/ws` (browsers cannot set that header on a WebSocket). There is no users table.

## Tests

```bash
cargo build --workspace
AIDB_CLI_BIN=./target/debug/aidb npm test
```

## Stack

- Vite + React + TypeScript
- Tailwind v4 + shadcn
- Same `app.db` the CLI opens. Keys stay out of the file.
