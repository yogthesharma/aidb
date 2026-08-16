# Harbor support desk — AIDB from a developer's chair

Two processes, one SQLite file. Not Studio. SQL never leaves the backend.

```text
frontend  ── JSON /api/* ──►  backend  ── AI.open ──►  desk.db
Vite :5174                    Fastify :8091
```

## Run it

From the repository root. Put the Moonshot key in `.env` (never in the file):

```
MOONSHOT_API_KEY=       # or KIMI_API_KEY
AIDB_LLM_MODEL=kimi-k2.5
```

Then:

```bash
pnpm example:support
```

Opens [http://127.0.0.1:5174](http://127.0.0.1:5174). Kimi Chat Completions is
`POST https://api.moonshot.ai/v1/chat/completions` with model `kimi-k2.5`.
Thinking is off unless `AIDB_KIMI_THINKING=1`. China keys:
`AIDB_KIMI_BASE_URL=https://api.moonshot.cn/v1`.

Without a key the same UI uses the fake provider.

Offline CI / no browser:

```bash
pnpm --filter harbor-backend demo
```

## Layout

```
examples/support/
  backend/     Fastify + AIDB (SQL lives here)
  frontend/    Vite UI (JSON only)
```
