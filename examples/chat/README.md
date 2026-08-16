# Chat — Ada on one empty AIDB file

Blank file. You type. Ada answers, streaming tokens from `run_events` as they
are written. Threads are `aidb_session` + the `session_turns` view. There is no
conversations table and no canned corpus.

```text
frontend  ── JSON /api/* ──►  backend  ── AI.open ──►  desk.db
Vite :5175                    Fastify :8092
```

## Run it

From the repository root. Put the Moonshot key in `.env` (never in the file):

```
MOONSHOT_API_KEY=       # or KIMI_API_KEY
AIDB_LLM_MODEL=kimi-k2.5
```

```bash
pnpm example:chat
```

Opens [http://127.0.0.1:5175](http://127.0.0.1:5175). Without a key it uses the
fake provider (echoes the prompt — enough to prove the file path).

Optional: **Knowledge** in the sidebar pastes *your* text into the file. After
that, questions run `aidb_generate … FROM aidb_search(…, 'chat')` against a
256-d embedding space (or OpenAI `text-embedding-3-small` if `OPENAI_API_KEY`
is set). Until then it is plain `aidb_generate`.

The left rail shows file spend, token counts, embed/index runs, process RSS,
and SQLite size. Those numbers are rows in the file plus `process.memoryUsage`.

Offline CI / no browser:

```bash
pnpm --filter chat-backend demo
```

## Layout

```
examples/chat/
  backend/     Fastify + AIDB (SQL lives here)
  frontend/    Vite chat UI (JSON only)
```
