# Security

AIDB stores application data, documents, embeddings, and durable AI runs in one
SQLite file. Provider keys are **never** stored in that file. They come from the
environment (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, …), then an optional secret
store (`AIDB_SECRET_STORE=keychain` or `file:/path`). The catalog stores a key
*name*, not the secret.

## Report a vulnerability

Please **do not** open a public issue for a security problem.

1. Use GitHub's [private vulnerability reporting](https://github.com/yogthesharma/aidb/security/advisories/new) on this repository.
2. Include the AIDB version / commit, the OS, and a minimal reproduction.
3. Do not attach a database file that contains production data.

We will acknowledge the report and work on a fix before any public disclosure.

## What is in scope

- Secrets or credentials written into `app.db`
- Unauthorized SQL / HTTP access when a bearer is configured
- Path traversal or unexpected file writes outside the opened database
- Crash-resume skipping an irreversible tool (HITL bypass)

## What is out of scope

- Using AIDB without a bearer on loopback (`aidb serve` is an inspect face, not a
  multi-tenant product)
- Prompt injection against a model you pointed AIDB at
- SQLite limits (database size, concurrent writers) that are documented engine
  constraints
- DataFusion / a second store (not shipped)

## Hardening tips

- Treat `app.db` like an application backup: encrypt it at rest if the host
  requires that.
- Set `AIDB_BEARER` if `aidb serve` is reachable beyond loopback.
- Keep provider keys in the environment or a secret store, never in SQL.
