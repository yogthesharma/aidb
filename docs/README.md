# AIDB documentation

Canonical product docs are in the repository root:

| Doc | What it is |
| --- | --- |
| [`DESIGN.md`](../DESIGN.md) | Architecture, IR, optimizer, product boundary |
| [`PHASES.md`](../PHASES.md) | Build order, SQL-first surface, what shipped |
| [`STATUS.md`](../STATUS.md) | Where v0 is: what works, what does not, JS/Python faces |

Guides in this folder:

| Guide | What it is |
| --- | --- |
| [Getting started](getting-started.md) | Clone, build, first file |
| [SQL surface](sql.md) | Functions, runs, search, agents, sessions, tokens |
| [HTTP and Studio](http.md) | `aidb serve` and the inspect face |
| [Projects](apps.md) | AI apps that fit the one-file model |
| [Stock desk](../examples/stock/README.md) | One real app on AIDB only (CLI) |
| [Support desk](../examples/support/README.md) | Product UI: frontend + AIDB backend |
| [Chat](../examples/chat/README.md) | ChatGPT-style UI on an empty AIDB file |

Contributing, security, and license:

- [`CONTRIBUTING.md`](../CONTRIBUTING.md)
- [`SECURITY.md`](../SECURITY.md)
- [`CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md)
- [`LICENSE`](../LICENSE)
