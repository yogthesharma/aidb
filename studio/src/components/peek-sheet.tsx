import { useEffect, useState } from "react";
import { Copy, Loader2, Search, SquareTerminal, X } from "lucide-react";
import { toast } from "sonner";
import { Hint } from "@/components/hint";
import { JsonView } from "@/components/json-view";
import { ResultGrid } from "@/components/result-grid";
import { StatusBadge } from "@/components/status-badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import {
  copyText,
  rowRecord,
  runSql,
  sqlIdent,
  type SqlResult,
} from "@/lib/aidb";
import { resumeSql, sqlString } from "@/lib/catalog.mjs";
import {
  displayCell,
  inferType,
  isNullish,
  looksJson,
  relativeMs,
} from "@/lib/format";
import { inferPeek, type PeekTarget } from "@/lib/peek";
import { cn } from "@/lib/utils";

export function PeekPane({
  target,
  onClose,
  onPeek,
  onOpenSql,
  onSearch,
  onChanged,
  revision = 0,
}: {
  target: PeekTarget;
  onClose: () => void;
  onPeek: (next: PeekTarget) => void;
  onOpenSql: (sql: string) => void;
  onSearch: (query: string) => void;
  onChanged: () => void;
  revision?: number;
}) {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [fields, setFields] = useState<Record<string, string>>({});
  const [related, setRelated] = useState<SqlResult | null>(null);
  const [relatedTitle, setRelatedTitle] = useState("Related");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    async function load() {
      setLoading(true);
      setError(null);
      setRelated(null);
      try {
        if (target.kind === "row") {
          if (!cancelled) {
            setFields(target.fields);
          }
          return;
        }
        if (target.kind === "document") {
          const [doc, chunks] = await Promise.all([
            runSql(
              `SELECT id, title, index_status, index_error, index_run_id, source_uri, content_hash, created_at_ms, updated_at_ms, metadata_json, length(content) AS bytes, content FROM documents WHERE id = ${sqlString(target.id)}`,
            ),
            runSql(
              `SELECT id, ordinal, token_count, length(content) AS bytes, substr(content, 1, 240) AS preview FROM chunks WHERE document_id = ${sqlString(target.id)} ORDER BY ordinal LIMIT 50`,
            ),
          ]);
          if (!doc.ok) {
            throw new Error(doc.error ?? "document lookup failed");
          }
          const row = doc.rows?.[0];
          if (!row || !doc.columns) {
            throw new Error("Document not found");
          }
          if (!cancelled) {
            setFields(rowRecord(doc.columns, row));
            setRelated(chunks);
            setRelatedTitle(`chunks (${chunks.rows?.length ?? 0})`);
          }
          return;
        }
        if (target.kind === "run") {
          const [run, events] = await Promise.all([
            runSql(
              `SELECT id, kind, status, document_id, parent_id, model, cost_usd, prompt_tokens, completion_tokens, error, created_at_ms, started_at_ms, finished_at_ms, input_json, output_json FROM runs WHERE id = ${sqlString(target.id)}`,
            ),
            runSql(
              `SELECT seq, kind, created_at_ms, substr(payload_json, 1, 280) AS payload FROM run_events WHERE run_id = ${sqlString(target.id)} ORDER BY seq LIMIT 80`,
            ),
          ]);
          if (!run.ok) {
            throw new Error(run.error ?? "run lookup failed");
          }
          const row = run.rows?.[0];
          if (!row || !run.columns) {
            throw new Error("Run not found");
          }
          if (!cancelled) {
            setFields(rowRecord(run.columns, row));
            setRelated(events);
            setRelatedTitle(`run_events (${events.rows?.length ?? 0})`);
          }
          return;
        }
        if (target.kind === "model") {
          const model = await runSql(
            `SELECT name, kind, provider, provider_model, key_name, dimensions, created_at_ms FROM models WHERE name = ${sqlString(target.name)}`,
          );
          if (!model.ok) {
            throw new Error(model.error ?? "model lookup failed");
          }
          const row = model.rows?.[0];
          if (!row || !model.columns) {
            throw new Error("Model not found");
          }
          if (!cancelled) {
            setFields(rowRecord(model.columns, row));
          }
          return;
        }
        const [info, sample, n] = await Promise.all([
          runSql(`PRAGMA table_info(${sqlIdent(target.name)})`),
          runSql(`SELECT * FROM ${sqlIdent(target.name)} LIMIT 50`),
          runSql(`SELECT COUNT(*) FROM ${sqlIdent(target.name)}`),
        ]);
        const colNames =
          info.ok && info.rows
            ? info.rows
                .map((row) => `${row[1]} ${row[2] ?? ""}`.trim())
                .join("\n")
            : (info.error ?? "");
        if (!cancelled) {
          setFields({
            name: target.name,
            rows: n.ok && n.rows?.[0] ? String(n.rows[0][0]) : "?",
            ddl: colNames,
          });
          setRelated(sample);
          setRelatedTitle(`SELECT * LIMIT 50`);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    }
    void load();
    return () => {
      cancelled = true;
    };
  }, [target, revision]);

  const title =
    target.kind === "document"
      ? fields.title && fields.title !== "NULL"
        ? fields.title
        : target.id
      : target.kind === "run"
        ? target.id
        : target.kind === "model"
          ? target.name
          : target.kind === "table"
            ? target.name
            : target.title;

  const copyId =
    target.kind === "document" || target.kind === "run"
      ? target.id
      : target.kind === "model" || target.kind === "table"
        ? target.name
        : null;

  async function resume(approved: boolean) {
    if (target.kind !== "run") {
      return;
    }
    setBusy(true);
    try {
      const result = await runSql(resumeSql(target.id, approved));
      if (!result.ok) {
        toast.error(result.error ?? "resume failed");
        return;
      }
      toast.success(approved ? "Run approved" : "Run rejected");
      onChanged();
      onPeek({ kind: "run", id: target.id });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-background">
      <div className="flex h-10 min-w-0 shrink-0 items-center gap-2 border-b px-3">
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium" title={title}>
          {title}
        </h2>
        <span className="max-w-[40%] shrink-0 truncate text-sm text-muted-foreground">
          {target.kind}
          {fields.status ? ` · ${fields.status}` : ""}
          {fields.index_status ? ` · ${fields.index_status}` : ""}
          {fields.kind && target.kind === "run" ? ` · ${fields.kind}` : ""}
        </span>
        <Hint label="Close details">
          <Button variant="ghost" size="icon-sm" className="shrink-0" onClick={onClose}>
            <X />
            <span className="sr-only">Close</span>
          </Button>
        </Hint>
      </div>
      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto overflow-x-hidden">
        {loading ? (
          <div className="flex items-center gap-2 p-4 text-sm text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" />
            Loading
          </div>
        ) : error ? (
          <p className="break-words p-4 text-sm text-destructive">{error}</p>
        ) : (
          <div className="flex min-w-0 flex-col">
            <FieldTable fields={fields} />
            {related ? (
              <>
                <Separator />
                <p className="px-3 py-2 text-sm text-muted-foreground">
                  {relatedTitle}
                </p>
                <div className="min-w-0 overflow-x-auto">
                  <ResultGrid
                    result={related}
                    emptyTitle="None"
                    emptyDescription="Empty."
                    fill={false}
                    contained
                    onRowClick={
                      target.kind === "table"
                        ? (row) => onPeek(inferPeek(row))
                        : undefined
                    }
                  />
                </div>
              </>
            ) : null}
          </div>
        )}
      </div>
      <div className="flex min-h-10 min-w-0 shrink-0 flex-row flex-wrap items-center justify-start gap-2 border-t px-3">
        {copyId ? (
          <Hint label="Copy id to clipboard">
            <Button
              variant="outline"
              size="sm"
              className="text-sm"
              onClick={() => {
                void copyText(copyId).then(() =>
                  toast.success("Copied to clipboard"),
                );
              }}
            >
              <Copy />
              Copy
            </Button>
          </Hint>
        ) : null}
        {target.kind === "document" ? (
          <Hint label="Search using this document title">
            <Button
              variant="outline"
              size="sm"
              className="text-sm"
              onClick={() =>
                onSearch(
                  fields.title && fields.title !== "NULL" ? fields.title : target.id,
                )
              }
            >
              <Search />
              Search
            </Button>
          </Hint>
        ) : null}
        {target.kind === "table" ? (
          <Hint label="SELECT * FROM this object LIMIT 50">
            <Button
              variant="outline"
              size="sm"
              className="text-sm"
              onClick={() =>
                onOpenSql(`SELECT * FROM ${sqlIdent(target.name)} LIMIT 50`)
              }
            >
              <SquareTerminal />
              SQL
            </Button>
          </Hint>
        ) : null}
        {target.kind === "document" &&
        fields.index_run_id &&
        fields.index_run_id !== "NULL" ? (
          <Hint label="Open the index run for this document">
            <Button
              variant="outline"
              size="sm"
              className="text-sm"
              onClick={() => onPeek({ kind: "run", id: fields.index_run_id })}
            >
              index_run
            </Button>
          </Hint>
        ) : null}
        {target.kind === "run" &&
        fields.document_id &&
        fields.document_id !== "NULL" ? (
          <Hint label="Open the linked document">
            <Button
              variant="outline"
              size="sm"
              className="text-sm"
              onClick={() => onPeek({ kind: "document", id: fields.document_id })}
            >
              document
            </Button>
          </Hint>
        ) : null}
        {target.kind === "run" && fields.status === "awaiting_approval" ? (
          <>
            <Hint label="SELECT aidb_resume(id, { approved: true })">
              <Button size="sm" disabled={busy} onClick={() => void resume(true)}>
                {busy ? <Loader2 className="animate-spin" /> : null}
                Approve
              </Button>
            </Hint>
            <Hint label="SELECT aidb_resume(id, { approved: false })">
              <Button
                variant="destructive"
                size="sm"
                disabled={busy}
                onClick={() => void resume(false)}
              >
                Reject
              </Button>
            </Hint>
          </>
        ) : null}
      </div>
    </div>
  );
}

function FieldTable({ fields }: { fields: Record<string, string> }) {
  return (
    <div className="min-w-0 overflow-x-auto">
    <table className="w-full min-w-0 table-fixed text-left text-sm">
      <thead>
        <tr className="border-b bg-muted/95 text-xs font-medium text-muted-foreground">
          <th className="w-[28%] px-3 py-2">Column</th>
          <th className="w-16 px-3 py-2">Type</th>
          <th className="px-3 py-2">Value</th>
        </tr>
      </thead>
      <tbody>
        {Object.entries(fields).map(([key, value]) => {
          const type = inferType(key, value);
          const rel = key.endsWith("_ms") ? relativeMs(value) : null;
          const json = looksJson(value);
          return (
            <tr key={key} className="group border-b border-border/50">
              <td className="px-3 py-2 align-top break-all text-muted-foreground">
                {key}
              </td>
              <td className="px-3 py-2 align-top text-muted-foreground">{type}</td>
              <td className="min-w-0 px-3 py-2 align-top">
                <div className="flex min-w-0 items-start gap-1">
                  <div className="min-w-0 flex-1 overflow-hidden">
                    {key === "status" || key === "index_status" ? (
                      <StatusBadge value={value} />
                    ) : json ? (
                      <JsonView value={value} />
                    ) : (
                      <span
                        className={cn(
                          "block max-w-full whitespace-pre-wrap break-all",
                          isNullish(value) && "italic text-muted-foreground/50",
                        )}
                      >
                        {displayCell(key, value)}
                        {rel ? (
                          <span className="ml-2 text-xs text-muted-foreground">
                            {rel}
                          </span>
                        ) : null}
                      </span>
                    )}
                  </div>
                  {!isNullish(value) ? (
                    <button
                      type="button"
                      className="mt-0.5 hidden shrink-0 text-muted-foreground hover:text-foreground group-hover:block"
                      onClick={() => {
                        void copyText(value).then(() =>
                          toast.success("Copied to clipboard"),
                        );
                      }}
                      title="Copy value"
                    >
                      <Copy className="size-3" />
                    </button>
                  ) : null}
                </div>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
    </div>
  );
}
