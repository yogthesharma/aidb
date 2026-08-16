# Stock desk — an AI application on one AIDB file

An equity research assistant with no AI framework behind it. Filings, embeddings,
answers with citations, agent steps, approvals, spend, and the desk's own watchlist
and signals tables all live in one SQLite file.

```bash
cargo build --workspace
node bindings/typescript/scripts/stage-native.mjs   # the napi addon

node examples/stock/stock.mjs demo --db /tmp/desk.db
```

The demo initializes the desk, ingests the corpus, answers a question with
citations, answers a second one scoped to a ticker and to an analyst's remembered
preference, classifies two headlines into a domain table, runs an agent brief, then
runs a digest that wants to email a client — which parks for approval instead of
sending — approves it, and prints the file as the report.

## Commands

```
init                      app tables, model, tool catalog, policy
ingest                    load the research corpus and the watchlist
ask "<question>"          cited answer   [--ticker AAPL] [--kind filing] [--user u1] [--k 4]
remember <user> "<text>"  store an analyst preference
brief <TICKER>            agent brief (search then generate)
digest <TICKER>           agent digest that wants to email — parks for approval
sentiment <TICKER> "<h>"  classify a headline into the signals table
waiting                   runs parked for a human
approve|reject <run_id>   resume a parked run
runs                      recent runs with cost
status                    file, documents, spend, parked runs
```

Add `--live` (with `OPENAI_API_KEY`) to use a real provider. The default path is
offline and deterministic, which is why CI can run it.

## The point

Everything the app needs to be durable is already durable, without the app doing
anything about it:

```bash
aidb sql /tmp/desk.db "SELECT id, kind, status, cost_usd FROM runs ORDER BY created_at_ms"
aidb sql /tmp/desk.db "SELECT w.ticker, COUNT(s.id) FROM watchlist w
                       LEFT JOIN signals s ON s.ticker = w.ticker GROUP BY w.ticker"
aidb runs /tmp/desk.db --waiting
```

Kill the process mid-digest and the approval is still there. Copy the file and you
copied the application's state, including its audit trail. There is no second store
to keep in sync, and no trace backend to pay for.

What the app is responsible for: its own tables, its own UI, and its own prompts.
What AIDB is responsible for: everything that has to survive.

[`NOTES.md`](NOTES.md) records what the engine was missing while this was built, and
which of those gaps became engine fixes rather than application code. The contract
this app relies on is tested in `crates/aidb/tests/stock_app.rs`, which also runs the
app itself end to end.
