import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import {
  Check,
  Loader2,
  Moon,
  Play,
  Plus,
  RefreshCw,
  Sun,
} from "lucide-react";
import { useTheme } from "next-themes";
import { toast } from "sonner";
import { AppSidebar } from "@/components/app-sidebar";
import { BearerButton, BearerDialog } from "@/components/bearer-dialog";
import { InsertDocumentDialog } from "@/components/insert-document-dialog";
import { PeekPane } from "@/components/peek-sheet";
import { ResultGrid } from "@/components/result-grid";
import { StatusBar } from "@/components/status-bar";
import { Hint } from "@/components/hint";
import { SqlEditor } from "@/components/sql-editor";
import { StudioBreadcrumb } from "@/components/studio-breadcrumb";
import { Toolbar } from "@/components/toolbar";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  cellText,
  firstNumber,
  metaMap,
  probe,
  runSqlTimed,
  type SqlResult,
} from "@/lib/aidb";
import { takeTokenFromUrl } from "@/lib/auth";
import { CATALOG_SQL, searchSql } from "@/lib/catalog.mjs";
import { connectLive } from "@/lib/live";
import { inferPeek, peekKey, type PeekTarget } from "@/lib/peek";
import {
  pageHref,
  parseStudioPath,
  peekHref,
  type StudioLocationState,
} from "@/lib/studio-path";

const SNIPPETS = [
  {
    id: "schema",
    label: "schema_version",
    sql: CATALOG_SQL.meta,
  },
  {
    id: "docs",
    label: "documents by status",
    sql: "SELECT index_status, COUNT(*) AS n FROM documents GROUP BY index_status",
  },
  {
    id: "failed",
    label: "failed runs",
    sql: "SELECT id, kind, status, error, created_at_ms FROM runs WHERE status = 'failed' ORDER BY created_at_ms DESC LIMIT 20",
  },
  {
    id: "hitl",
    label: "awaiting_approval",
    sql: "SELECT id, kind, status, created_at_ms FROM runs WHERE status = 'awaiting_approval'",
  },
  {
    id: "search",
    label: "aidb_search",
    sql: searchSql("How do refunds work?", 5),
  },
  {
    id: "experiments",
    label: "experiment_results",
    sql: CATALOG_SQL.experiments,
  },
  {
    id: "tokens",
    label: "generate tokens",
    sql: CATALOG_SQL.tokens,
  },
  {
    id: "models",
    label: "models",
    sql: CATALOG_SQL.models,
  },
] as const;

export default function App() {
  const location = useLocation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const parsed = parseStudioPath(location.pathname);
  const page = parsed.page;
  const rowPeek =
    page === "sql" ? ((location.state as StudioLocationState | null)?.rowPeek ?? null) : null;
  const peek = parsed.peek ?? rowPeek;

  const [online, setOnline] = useState<boolean | null>(null);
  const [loading, setLoading] = useState(false);
  const [sql, setSql] = useState<string>(SNIPPETS[0].sql);
  const [sqlResult, setSqlResult] = useState<SqlResult | null>(null);
  const [running, setRunning] = useState(false);
  const [meta, setMeta] = useState<SqlResult | null>(null);
  const [tables, setTables] = useState<SqlResult | null>(null);
  const [docs, setDocs] = useState<SqlResult | null>(null);
  const [runs, setRuns] = useState<SqlResult | null>(null);
  const [models, setModels] = useState<SqlResult | null>(null);
  const [experiments, setExperiments] = useState<SqlResult | null>(null);
  const [counts, setCounts] = useState<{
    documents: number | null;
    runs: number | null;
    models: number | null;
    waiting: number | null;
    experiments: number | null;
  }>({ documents: null, runs: null, models: null, waiting: null, experiments: null });
  const [searchQ, setSearchQ] = useState("How do refunds work?");
  const [searchK, setSearchK] = useState("5");
  const [searchResult, setSearchResult] = useState<SqlResult | null>(null);
  const [searching, setSearching] = useState(false);
  const [insertOpen, setInsertOpen] = useState(false);
  const [bearerOpen, setBearerOpen] = useState(false);
  const [needsBearer, setNeedsBearer] = useState(false);
  const [authGen, setAuthGen] = useState(0);
  const [lastMs, setLastMs] = useState<number | null>(null);
  const [lastRows, setLastRows] = useState<number | null>(null);
  const [lastError, setLastError] = useState<string | null>(null);
  const [liveTick, setLiveTick] = useState(0);

  const noteTiming = useCallback((ms: number, result: SqlResult) => {
    setLastMs(ms);
    setLastRows(result.ok && result.rows ? result.rows.length : null);
    setLastError(result.ok ? null : (result.error ?? "error"));
  }, []);

  const refresh = useCallback(async (quiet = false) => {
    if (!quiet) {
      setLoading(true);
    }
    try {
      const [
        metaRows,
        tableRows,
        docRows,
        runRows,
        modelRows,
        experimentRows,
        nDocs,
        nRuns,
        nModels,
        nWait,
        nExperiments,
      ] = await Promise.all([
        runSqlTimed(CATALOG_SQL.meta),
        runSqlTimed(CATALOG_SQL.tables),
        runSqlTimed(CATALOG_SQL.documents),
        runSqlTimed(CATALOG_SQL.runs),
        runSqlTimed(CATALOG_SQL.models),
        runSqlTimed(CATALOG_SQL.experiments),
        runSqlTimed(CATALOG_SQL.nDocuments),
        runSqlTimed(CATALOG_SQL.nRuns),
        runSqlTimed(CATALOG_SQL.nModels),
        runSqlTimed(CATALOG_SQL.nWaiting),
        runSqlTimed(CATALOG_SQL.nExperiments),
      ]);
      setMeta(metaRows.result);
      setTables(tableRows.result);
      setDocs(docRows.result);
      setRuns(runRows.result);
      setModels(modelRows.result);
      setExperiments(experimentRows.result);
      setCounts({
        documents: firstNumber(nDocs.result),
        runs: firstNumber(nRuns.result),
        models: firstNumber(nModels.result),
        waiting: firstNumber(nWait.result),
        experiments: firstNumber(nExperiments.result),
      });
      setLiveTick((tick) => tick + 1);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    takeTokenFromUrl();
    const onAuth = () => setAuthGen((gen) => gen + 1);
    window.addEventListener("aidb-auth", onAuth);
    return () => window.removeEventListener("aidb-auth", onAuth);
  }, []);

  useEffect(() => {
    let debounce: number | null = null;
    let stopLive: (() => void) | null = null;
    let cancelled = false;

    async function boot() {
      const reach = await probe();
      if (cancelled) {
        return;
      }
      if (reach === "unauthorized") {
        setNeedsBearer(true);
        setOnline(false);
        return;
      }
      setNeedsBearer(false);
      if (reach === "down") {
        setOnline(false);
        return;
      }
      stopLive = connectLive({
        onStatus: (ok) => setOnline(ok),
        onEvent: (message) => {
          if (message.type !== "hello" && message.type !== "change" && message.type !== "token") {
            return;
          }
          if (debounce !== null) {
            window.clearTimeout(debounce);
          }
          debounce = window.setTimeout(() => {
            void refresh(true);
          }, message.type === "hello" ? 0 : 120);
        },
      });
    }

    void boot();
    return () => {
      cancelled = true;
      if (debounce !== null) {
        window.clearTimeout(debounce);
      }
      stopLive?.();
    };
  }, [refresh, authGen]);

  const lastOnline = useRef<boolean | null>(null);

  useEffect(() => {
    if (online === null) {
      return;
    }
    if (lastOnline.current === false && online) {
      toast.success("aidb serve is reachable");
    }
    if (lastOnline.current === true && !online) {
      toast.error("aidb serve went away");
    }
    lastOnline.current = online;
  }, [online]);

  useEffect(() => {
    if (online) {
      void refresh();
    }
  }, [online, refresh]);

  useEffect(() => {
    if (!parsed.known) {
      navigate("/file", { replace: true });
    }
  }, [parsed.known, navigate]);

  const execute = useCallback(
    async (statement: string) => {
      setRunning(true);
      try {
        const { result, ms } = await runSqlTimed(statement);
        setSqlResult(result);
        noteTiming(ms, result);
        if (result.ok) {
          toast.success(
            result.columns
              ? `${result.rows?.length ?? 0} rows in ${ms}ms`
              : `Changed ${result.changed ?? 0} · ${ms}ms`,
          );
          void refresh();
        } else {
          toast.error(result.error ?? "Query failed");
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setSqlResult({ ok: false, error: message });
        setLastError(message);
        toast.error(message);
      } finally {
        setRunning(false);
      }
    },
    [noteTiming, refresh],
  );

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter" && page === "sql") {
        event.preventDefault();
        if (!running && online) {
          void execute(sql);
        }
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [page, sql, running, online, execute]);

  const runSearch = useCallback(
    async (query = searchQ) => {
      setSearching(true);
      try {
        const { result, ms } = await runSqlTimed(searchSql(query, searchK));
        setSearchResult(result);
        noteTiming(ms, result);
        if (!result.ok) {
          toast.error(result.error ?? "Search failed");
        } else {
          toast.success(`${result.rows?.length ?? 0} hits in ${ms}ms`);
        }
      } finally {
        setSearching(false);
      }
    },
    [noteTiming, searchK, searchQ],
  );

  function goPeek(target: PeekTarget) {
    if (target.kind === "row") {
      navigate("/sql", { state: { rowPeek: target } satisfies StudioLocationState });
      return;
    }
    navigate(peekHref(target));
  }

  function closePeek() {
    navigate(pageHref(page));
  }

  function openSql(statement: string) {
    setSql(statement);
    navigate("/sql");
    void execute(statement);
  }

  function openSearch(query: string) {
    setSearchQ(query);
    navigate("/search");
    void runSearch(query);
  }

  const schema = useMemo(() => metaMap(meta).get("schema_version") ?? "—", [meta]);
  const runFilter = searchParams.get("status") === "waiting" ? "waiting" : "all";
  const filteredRuns = useMemo(
    () => (runFilter === "waiting" ? filterStatus(runs, "awaiting_approval") : runs),
    [runs, runFilter],
  );
  const tableList = useMemo(() => catalogTables(tables), [tables]);

  return (
    <SidebarProvider className="h-svh min-h-0 overflow-hidden">
      <AppSidebar
        page={page}
        waiting={counts.waiting}
        documents={counts.documents}
        experiments={counts.experiments}
        tables={tableList}
        tableName={peek?.kind === "table" ? peek.name : null}
      />
      <SidebarInset className="min-h-0 overflow-hidden">
        <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
          <SidebarTrigger />
          <Separator
            orientation="vertical"
            className="mx-1 h-4 self-center data-vertical:h-4 data-vertical:self-center"
          />
          <StudioBreadcrumb page={page} peek={peek} />
          <div className="ml-auto flex items-center gap-0.5">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => {
                    void refresh().then(() => toast.success("Catalog reloaded"));
                  }}
                  disabled={!online || loading}
                >
                  <RefreshCw className={loading ? "animate-spin" : undefined} />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Reload catalog</TooltipContent>
            </Tooltip>
            <BearerButton locked={needsBearer} onClick={() => setBearerOpen(true)} />
            <ThemeMenu />
          </div>
        </header>

        <ResizablePanelGroup
          orientation="horizontal"
          className="min-h-0 min-w-0 flex-1"
          id="studio-page"
        >
          <ResizablePanel id="studio-main" minSize={28} defaultSize={peek ? 58 : 100} className="min-w-0">
            <div className="flex h-full min-h-0 flex-col overflow-hidden">
              {needsBearer ? (
                <div className="p-3">
                  <Alert variant="destructive">
                    <AlertTitle className="text-sm">bearer required</AlertTitle>
                    <AlertDescription className="text-xs">
                      This serve is gated by <code>AIDB_BEARER</code>. Set the same
                      token in the key icon — Studio sends it on <code>POST /sql</code>{" "}
                      and <code>/ws?token=</code>.
                    </AlertDescription>
                  </Alert>
                </div>
              ) : online === false ? (
                <div className="p-3">
                  <Alert variant="destructive">
                    <AlertTitle className="text-sm">serve unreachable</AlertTitle>
                    <AlertDescription className="text-xs">
                      cargo run -p aidb-cli -- serve ./app.db
                    </AlertDescription>
                  </Alert>
                </div>
              ) : null}

              {page === "overview" ? (
                <Overview
                  meta={meta}
                  tables={tables}
                  loading={loading}
                  schema={schema}
                  counts={counts}
                  onPeek={goPeek}
                />
              ) : null}
              {page === "sql" ? (
                <SqlConsole
                  sql={sql}
                  setSql={setSql}
                  running={running}
                  online={!!online}
                  result={sqlResult}
                  lastMs={lastMs}
                  selectedKey={peek ? peekKey(peek) : null}
                  onRun={() => void execute(sql)}
                  onPeek={goPeek}
                />
              ) : null}
              {page === "documents" ? (
                <PageBody
                  result={docs}
                  loading={loading}
                  statusColumn="index_status"
                  selectedKey={peek ? peekKey(peek) : null}
                  onRowClick={(row) => goPeek(inferPeek(row))}
                  emptyTitle="No documents"
                  emptyDescription="INSERT via aidb_insert_document"
                  actions={
                    <Hint label="INSERT via aidb_insert_document">
                      <Button onClick={() => setInsertOpen(true)} disabled={!online}>
                        <Plus />
                        Insert
                      </Button>
                    </Hint>
                  }
                />
              ) : null}
              {page === "search" ? (
                <SearchPlayground
                  query={searchQ}
                  setQuery={setSearchQ}
                  k={searchK}
                  setK={setSearchK}
                  result={searchResult}
                  searching={searching}
                  online={!!online}
                  selectedKey={peek ? peekKey(peek) : null}
                  onRun={() => void runSearch()}
                  onPeek={goPeek}
                  onAddDocument={() => {
                    navigate("/documents");
                    setInsertOpen(true);
                  }}
                />
              ) : null}
              {page === "runs" ? (
                <PageBody
                  result={filteredRuns}
                  loading={loading}
                  statusColumn="status"
                  selectedKey={peek ? peekKey(peek) : null}
                  onRowClick={(row) => goPeek(inferPeek(row))}
                  emptyTitle={runFilter === "waiting" ? "None awaiting_approval" : "No runs"}
                  emptyDescription="Runs are rows in this file."
                  actions={
                    <Select
                      value={runFilter}
                      onValueChange={(value) => {
                        if (value === "waiting") {
                          setSearchParams({ status: "waiting" });
                        } else {
                          setSearchParams({});
                        }
                      }}
                    >
                      <SelectTrigger className="w-48">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="all">all</SelectItem>
                        <SelectItem value="waiting">awaiting_approval</SelectItem>
                      </SelectContent>
                    </Select>
                  }
                />
              ) : null}
              {page === "experiments" ? (
                <PageBody
                  result={experiments}
                  loading={loading}
                  statusColumn="status"
                  selectedKey={peek ? peekKey(peek) : null}
                  onRowClick={(row) => goPeek(inferPeek(row))}
                  emptyTitle="No experiments"
                  emptyDescription="SELECT aidb_experiment(...) writes these rows."
                />
              ) : null}
              {page === "models" ? (
                <PageBody
                  result={models}
                  loading={loading}
                  selectedKey={peek ? peekKey(peek) : null}
                  onRowClick={(row) => goPeek(inferPeek(row))}
                  emptyTitle="No models"
                  emptyDescription="CREATE MODEL · key_name only"
                />
              ) : null}
            </div>
          </ResizablePanel>
          {peek ? (
            <>
              <ResizableHandle withHandle />
              <ResizablePanel
                id="studio-detail"
                minSize={22}
                defaultSize={42}
                className="min-w-0"
              >
                <PeekPane
                  target={peek}
                  revision={liveTick}
                  onClose={closePeek}
                  onPeek={goPeek}
                  onOpenSql={openSql}
                  onSearch={openSearch}
                  onChanged={() => void refresh()}
                />
              </ResizablePanel>
            </>
          ) : null}
        </ResizablePanelGroup>
        <StatusBar
          online={online}
          live={online === true}
          schema={schema}
          lastMs={lastMs}
          lastRows={lastRows}
          lastError={lastError}
          documents={counts.documents}
          runs={counts.runs}
          waiting={counts.waiting}
          experiments={counts.experiments}
        />
      </SidebarInset>
      <InsertDocumentDialog
        open={insertOpen}
        onOpenChange={setInsertOpen}
        onInserted={(id) => {
          void refresh();
          goPeek({ kind: "document", id });
        }}
      />
      <BearerDialog open={bearerOpen} onOpenChange={setBearerOpen} />
    </SidebarProvider>
  );
}

function catalogTables(result: SqlResult | null): { name: string; type: string }[] {
  if (!result?.ok || !result.rows) {
    return [];
  }
  return result.rows.map((row) => ({
    name: cellText(row[0]),
    type: cellText(row[1]),
  }));
}

function filterStatus(result: SqlResult | null, status: string): SqlResult | null {
  if (!result?.ok || !result.columns || !result.rows) {
    return result;
  }
  const index = result.columns.indexOf("status");
  if (index < 0) {
    return result;
  }
  return {
    ...result,
    rows: result.rows.filter((row) => cellText(row[index]) === status),
  };
}

function ThemeMenu() {
  const { theme, setTheme } = useTheme();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label="Color theme">
          <Sun className="dark:hidden" />
          <Moon className="hidden dark:block" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {(["light", "dark", "system"] as const).map((value) => (
          <DropdownMenuItem key={value} onClick={() => setTheme(value)}>
            {theme === value ? <Check /> : <span className="size-4" />}
            {value}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Overview({
  meta,
  tables,
  loading,
  schema,
  counts,
  onPeek,
}: {
  meta: SqlResult | null;
  tables: SqlResult | null;
  loading: boolean;
  schema: string;
  counts: {
    documents: number | null;
    runs: number | null;
    models: number | null;
    waiting: number | null;
    experiments: number | null;
  };
  onPeek: (target: PeekTarget) => void;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="grid h-10 shrink-0 grid-cols-2 border-b sm:grid-cols-3 lg:grid-cols-6">
        {(
          [
            ["Schema", schema, null],
            ["Documents", counts.documents ?? "—", "/documents"],
            ["Runs", counts.runs ?? "—", "/runs"],
            ["Models", counts.models ?? "—", "/models"],
            ["Experiments", counts.experiments ?? "—", "/experiments"],
            ["Waiting", counts.waiting ?? "—", "/runs?status=waiting"],
          ] as const
        ).map(([label, value, href]) => {
          const inner = (
            <>
              <span className="text-xs text-muted-foreground">{label}</span>
              <span className="text-sm font-medium tabular-nums">{value}</span>
            </>
          );
          const className =
            "flex h-10 items-center justify-between gap-2 border-r px-3 last:border-r-0";
          return href ? (
            <Link key={label} to={href} className={`${className} hover:bg-muted/40`}>
              {inner}
            </Link>
          ) : (
            <div key={label} className={className}>
              {inner}
            </div>
          );
        })}
      </div>
      <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
        <ResizablePanel defaultSize={42} minSize={24}>
          <ResultGrid
            result={meta}
            loading={loading}
            leading={<span className="text-sm text-muted-foreground">aidb_meta</span>}
            onRowClick={(row) => onPeek(inferPeek(row))}
          />
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel defaultSize={58} minSize={24}>
          <ResultGrid
            result={tables}
            loading={loading}
            leading={
              <span className="text-sm text-muted-foreground">
                Objects — click to inspect
              </span>
            }
            onRowClick={(row) => onPeek(inferPeek(row))}
          />
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}

function SqlConsole({
  sql,
  setSql,
  running,
  online,
  result,
  lastMs,
  selectedKey,
  onRun,
  onPeek,
}: {
  sql: string;
  setSql: (sql: string) => void;
  running: boolean;
  online: boolean;
  result: SqlResult | null;
  lastMs: number | null;
  selectedKey: string | null;
  onRun: () => void;
  onPeek: (target: PeekTarget) => void;
}) {
  return (
    <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
      <ResizablePanel defaultSize={32} minSize={16}>
        <div className="flex h-full min-h-0 flex-col">
          <Toolbar>
            <Hint label="Load a sample query against this file">
              <div>
                <Select
                  value={SNIPPETS.find((item) => item.sql === sql)?.id ?? ""}
                  onValueChange={(id) => {
                    const snippet = SNIPPETS.find((item) => item.id === id);
                    if (snippet) {
                      setSql(snippet.sql);
                      toast.message(`Loaded “${snippet.label}”`);
                    }
                  }}
                >
                  <SelectTrigger className="w-48">
                    <SelectValue placeholder="Snippets" />
                  </SelectTrigger>
                  <SelectContent>
                    {SNIPPETS.map((item) => (
                      <SelectItem key={item.id} value={item.id}>
                        {item.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </Hint>
            <Hint label="Run POST /sql on this file">
              <Button onClick={onRun} disabled={running || !online}>
                {running ? <Loader2 className="animate-spin" /> : <Play />}
                Run
              </Button>
            </Hint>
            <Hint label="⌘ Enter also runs the statement">
              <span className="ml-auto hidden items-center gap-1.5 text-sm text-muted-foreground sm:flex">
                <Kbd>⌘</Kbd>
                <Kbd>Enter</Kbd>
                {lastMs !== null ? (
                  <span className="ml-2 tabular-nums">{lastMs}ms</span>
                ) : null}
              </span>
            </Hint>
          </Toolbar>
          <SqlEditor value={sql} onChange={setSql} />
        </div>
      </ResizablePanel>
      <ResizableHandle />
      <ResizablePanel defaultSize={68} minSize={28}>
        <ResultGrid
          result={result}
          selectedKey={selectedKey}
          emptyTitle="No result"
          emptyDescription="⌘Enter runs POST /sql on this file."
          onRowClick={(row) => onPeek(inferPeek(row))}
        />
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}

function SearchPlayground({
  query,
  setQuery,
  k,
  setK,
  result,
  searching,
  online,
  selectedKey,
  onRun,
  onPeek,
  onAddDocument,
}: {
  query: string;
  setQuery: (value: string) => void;
  k: string;
  setK: (value: string) => void;
  result: SqlResult | null;
  searching: boolean;
  online: boolean;
  selectedKey: string | null;
  onRun: () => void;
  onPeek: (target: PeekTarget) => void;
  onAddDocument: () => void;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <Toolbar>
        <Hint label="Passed to aidb_search as the query">
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search query"
            className="h-8 w-72 shrink-0"
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                onRun();
              }
            }}
          />
        </Hint>
        <Hint label="Number of hits to return">
          <div>
            <Select value={k} onValueChange={setK}>
              <SelectTrigger className="w-28">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {["3", "5", "10", "20"].map((value) => (
                  <SelectItem key={value} value={value}>
                    k={value}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </Hint>
        <Hint label="Run aidb_search on this file">
          <Button onClick={onRun} disabled={searching || !online}>
            {searching ? <Loader2 className="animate-spin" /> : <Play />}
            Search
          </Button>
        </Hint>
      </Toolbar>
      <ResultGrid
        result={result}
        loading={searching}
        selectedKey={selectedKey}
        emptyTitle="No hits"
        emptyDescription="Index a document, then search."
        emptyAction={
          <Button variant="outline" onClick={onAddDocument}>
            <Plus />
            Insert
          </Button>
        }
        onRowClick={(row) => onPeek(inferPeek(row))}
      />
    </div>
  );
}

function PageBody({
  result,
  loading,
  statusColumn,
  selectedKey,
  onRowClick,
  emptyTitle,
  emptyDescription,
  actions,
}: {
  result: SqlResult | null;
  loading: boolean;
  statusColumn?: string;
  selectedKey?: string | null;
  onRowClick?: (row: Record<string, string>) => void;
  emptyTitle?: string;
  emptyDescription?: string;
  actions?: ReactNode;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <ResultGrid
        result={result}
        loading={loading}
        statusColumn={statusColumn}
        filterable
        selectedKey={selectedKey}
        onRowClick={onRowClick}
        emptyTitle={emptyTitle}
        emptyDescription={emptyDescription}
        leading={actions}
      />
    </div>
  );
}
