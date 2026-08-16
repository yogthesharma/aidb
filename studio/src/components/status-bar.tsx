import { Link } from "react-router";
import { Hint } from "@/components/hint";
import { VRule } from "@/components/toolbar";
import { cn } from "@/lib/utils";

export function StatusBar({
  online,
  live = false,
  schema,
  lastMs,
  lastRows,
  lastError,
  documents,
  runs,
  waiting,
  experiments,
}: {
  online: boolean | null;
  live?: boolean;
  schema: string;
  lastMs: number | null;
  lastRows: number | null;
  lastError: string | null;
  documents: number | null;
  runs: number | null;
  waiting: number | null;
  experiments?: number | null;
}) {
  return (
    <footer className="flex h-9 min-w-0 shrink-0 items-center gap-2 overflow-hidden border-t bg-muted/20 px-3 text-sm text-muted-foreground">
      <Hint
        label={
          live
            ? "WebSocket /ws to the same aidb serve"
            : online === false
              ? "Start: cargo run -p aidb-cli -- serve ./app.db"
              : "Connecting to /ws"
        }
        side="top"
      >
        <span className="flex cursor-default items-center gap-2">
          <span
            className={cn(
              "size-2 shrink-0 rounded-full",
              live ? "bg-emerald-500" : online === false ? "bg-red-500" : "bg-zinc-500",
            )}
          />
          {live ? "live · 127.0.0.1:8080" : online === false ? "serve down" : "connecting"}
        </span>
      </Hint>
      <VRule />
      <Hint label="aidb_meta.schema_version in this file" side="top">
        <span className="cursor-default">schema {schema}</span>
      </Hint>
      <VRule />
      <Hint label="Studio talks to the same Aidb process as the CLI" side="top">
        <span className="cursor-default">POST /sql</span>
      </Hint>
      {lastMs !== null ? (
        <>
          <VRule />
          <Hint label="Last query round-trip" side="top">
            <span className="cursor-default tabular-nums text-foreground">
              {formatMaybe(lastMs)}
            </span>
          </Hint>
        </>
      ) : null}
      {lastRows !== null ? (
        <>
          <VRule />
          <Hint label="Rows in the last result" side="top">
            <span className="cursor-default tabular-nums">{lastRows} rows</span>
          </Hint>
        </>
      ) : null}
      {lastError ? (
        <>
          <VRule />
          <Hint label={lastError} side="top">
            <span className="min-w-0 cursor-default truncate text-destructive">
              {lastError}
            </span>
          </Hint>
        </>
      ) : null}
      <span className="ml-auto flex shrink-0 items-center gap-3 tabular-nums">
        <Hint label="COUNT(*) FROM documents" side="top">
          <Link to="/documents" className="hover:text-foreground">
            {documents ?? "—"} docs
          </Link>
        </Hint>
        <Hint label="COUNT(*) FROM runs" side="top">
          <Link to="/runs" className="hover:text-foreground">
            {runs ?? "—"} runs
          </Link>
        </Hint>
        <Hint label="COUNT(*) FROM experiment_results" side="top">
          <Link to="/experiments" className="hover:text-foreground">
            {experiments ?? "—"} evals
          </Link>
        </Hint>
        {waiting ? (
          <Hint label="runs.status = awaiting_approval" side="top">
            <Link to="/runs?status=waiting" className="text-amber-500 hover:text-amber-400">
              {waiting} waiting
            </Link>
          </Hint>
        ) : null}
      </span>
    </footer>
  );
}

function formatMaybe(ms: number): string {
  if (ms < 1000) {
    return `${ms}ms`;
  }
  return `${(ms / 1000).toFixed(2)}s`;
}
