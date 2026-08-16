# HTTP and Studio

`aidb serve` is HTTP in front of the same `Aidb` and the same file. Optional. The
embedded file remains the product. CLI `sql` and the bindings do not need the
server.

## Serve

```bash
cargo run -p aidb-cli -- serve ./app.db
# listens on http://127.0.0.1:8080
```

| Method | Path | What it is |
| --- | --- | --- |
| `POST` | `/sql` | Body is a SQL statement. Same engine as `aidb sql`. |
| `GET` | `/health` | Process is up. |
| `GET` | `/ws` | Catalog change + generate `token` frames. |
| `GET` | `/runs/{id}/events` | `run_events` for that run (tokens included). |

Protect it with a bearer if it is not loopback-only:

```bash
AIDB_BEARER=secret cargo run -p aidb-cli -- serve ./app.db
# Authorization: Bearer secret
# WebSocket: GET /ws?token=secret  (browsers cannot set that header)
```

There is no users table. A bearer is a header, not an identity product.

## Studio

Studio is the inspect face over that server. Pages are `SELECT`s from
`studio/src/lib/catalog.mjs`. Approve is `SELECT aidb_resume(...)`.

```bash
cargo run -p aidb-cli -- serve ./app.db   # terminal 1
cd studio && npm install && npm run dev   # terminal 2
# http://127.0.0.1:5173  proxies /sql /health /ws to :8080
```

See [`studio/README.md`](../studio/README.md) for bearer injection and tests.

Product UIs that do **not** speak SQL from the browser: [`examples/support`](../examples/support/README.md) and [`examples/chat`](../examples/chat/README.md) (Fastify `/api/*` + Vite). Studio remains the inspect face over `aidb serve`.

Live generate tokens arrive as WebSocket `{"type":"token",…}`. The prefix in the
file is still `run_events`; the socket is how the UI refreshes while the write
mutex holds the connection.
