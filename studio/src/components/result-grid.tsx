import { useMemo, useState, type KeyboardEvent, type ReactNode } from "react";
import { DatabaseZap } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Toolbar } from "@/components/toolbar";
import { cellText, rowRecord, type SqlResult } from "@/lib/aidb";
import { cellTitle, displayCell, inferType, isNullish } from "@/lib/format";
import { inferPeek, peekKey } from "@/lib/peek";
import { StatusBadge } from "@/components/status-badge";
import { cn } from "@/lib/utils";

export function ResultGrid({
  result,
  loading = false,
  emptyTitle = "No rows",
  emptyDescription = "Run SQL against the same file the CLI opens.",
  emptyAction,
  statusColumn,
  onRowClick,
  selectedKey,
  rowKey,
  filterable = false,
  filterPlaceholder = "Filter",
  fill = true,
  contained = false,
  leading,
}: {
  result: SqlResult | null;
  loading?: boolean;
  emptyTitle?: string;
  emptyDescription?: string;
  emptyAction?: ReactNode;
  statusColumn?: string;
  onRowClick?: (row: Record<string, string>) => void;
  selectedKey?: string | null;
  rowKey?: (row: Record<string, string>) => string;
  filterable?: boolean;
  filterPlaceholder?: string;
  fill?: boolean;
  contained?: boolean;
  leading?: ReactNode;
}) {
  const [filter, setFilter] = useState("");
  const clickable = Boolean(onRowClick);

  const rows = useMemo(() => {
    if (!result?.ok || !result.columns || !result.rows) {
      return [];
    }
    const mapped = result.rows.map((raw) => ({
      raw,
      record: rowRecord(result.columns ?? [], raw),
    }));
    const needle = filter.trim().toLowerCase();
    if (!needle) {
      return mapped;
    }
    return mapped.filter(({ record }) =>
      Object.values(record).some((value) => value.toLowerCase().includes(needle)),
    );
  }, [result, filter]);

  const bar =
    leading || filterable || (result?.ok && result.columns) ? (
      <Toolbar className={cn(contained && "min-w-0 overflow-hidden")}>
        {leading}
        {filterable ? (
          <Input
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder={filterPlaceholder}
            className="h-8 w-56 shrink-0"
          />
        ) : null}
        {result?.ok && result.columns ? (
          <span className="ml-auto shrink-0 text-sm tabular-nums text-muted-foreground">
            {result.columns.length} cols
            <span className="mx-2 text-border">·</span>
            {filter ? `${rows.length} / ${result.rows?.length ?? 0}` : result.rows?.length ?? 0}{" "}
            rows
          </span>
        ) : (
          <span className="ml-auto" />
        )}
      </Toolbar>
    ) : null;

  if (loading && !result) {
    return (
      <div className={cn("flex min-h-0 flex-col", fill && "flex-1")}>
        {bar}
        <div className="flex flex-col gap-2 p-3">
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-2/3" />
        </div>
      </div>
    );
  }
  if (!result) {
    return (
      <div className={cn("flex min-h-0 flex-col", fill && "flex-1")}>
        {bar}
        <Empty className="min-h-0 flex-1 border-0">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <DatabaseZap />
            </EmptyMedia>
            <EmptyTitle>{emptyTitle}</EmptyTitle>
            <EmptyDescription>{emptyDescription}</EmptyDescription>
          </EmptyHeader>
          {emptyAction ? <EmptyContent>{emptyAction}</EmptyContent> : null}
        </Empty>
      </div>
    );
  }
  if (!result.ok) {
    return (
      <div className={cn("flex min-h-0 flex-col", fill && "flex-1")}>
        {bar}
        <Alert variant="destructive" className="m-3">
          <AlertTitle>Query failed</AlertTitle>
          <AlertDescription className="text-sm">{result.error}</AlertDescription>
        </Alert>
      </div>
    );
  }
  if (!result.columns) {
    return (
      <div className={cn("flex min-h-0 flex-col", fill && "flex-1")}>
        {bar}
        <p className="p-3 text-sm text-muted-foreground">
          OK · changed {result.changed ?? 0}
        </p>
      </div>
    );
  }
  if (!result.rows?.length) {
    return (
      <div className={cn("flex min-h-0 flex-col", fill && "flex-1")}>
        {bar}
        <Empty className="min-h-0 flex-1 border-0">
          <EmptyHeader>
            <EmptyTitle>0 rows</EmptyTitle>
            <EmptyDescription>{emptyDescription}</EmptyDescription>
          </EmptyHeader>
          {emptyAction ? <EmptyContent>{emptyAction}</EmptyContent> : null}
        </Empty>
      </div>
    );
  }

  const statusIndex = statusColumn
    ? result.columns.findIndex((column) => column === statusColumn)
    : -1;
  const types = result.columns.map((column, index) =>
    inferType(column, cellText(rows[0]?.raw[index])),
  );

  function onKey(event: KeyboardEvent<HTMLTableRowElement>, record: Record<string, string>) {
    if (!onRowClick) {
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onRowClick(record);
    }
  }

  return (
    <div className={cn("flex min-h-0 flex-col", fill && "flex-1")}>
      {bar}
      {filter && rows.length === 0 ? (
        <p className="p-3 text-sm text-muted-foreground">No match for “{filter}”</p>
      ) : (
        <div className={cn("min-h-0 overflow-auto", fill && "flex-1")}>
          <table className={cn(
            "border-separate border-spacing-0 text-left text-sm",
            contained ? "w-full min-w-0" : "w-max min-w-full",
          )}>
            <thead className="sticky top-0 z-10">
              <tr>
                <th className="sticky left-0 z-20 w-12 border-b bg-muted/95 px-3 py-2 text-right text-xs font-medium text-muted-foreground backdrop-blur">
                  #
                </th>
                {result.columns.map((column, index) => (
                  <th
                    key={column}
                    className={cn(
                      "border-b bg-muted/95 px-3 py-2 text-xs font-medium backdrop-blur",
                      contained && "max-w-[14rem] truncate",
                    )}
                    title={`${column} · ${types[index]}`}
                  >
                    <span className="text-foreground">{column}</span>
                    <span className="ml-1.5 font-normal text-muted-foreground">
                      {types[index]}
                    </span>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map(({ raw, record }, index) => {
                const key = rowKey?.(record) ?? peekKey(inferPeek(record));
                return (
                  <tr
                    key={`${key}-${index}`}
                    tabIndex={clickable ? 0 : undefined}
                    data-state={selectedKey === key ? "selected" : undefined}
                    className={cn(
                      "group h-9",
                      clickable && "cursor-pointer focus-visible:outline-none",
                      "hover:bg-muted/40 data-[state=selected]:bg-primary/10",
                    )}
                    onClick={() => onRowClick?.(record)}
                    onKeyDown={(event) => onKey(event, record)}
                  >
                    <td className="sticky left-0 z-10 w-12 border-b border-border/50 bg-background px-3 text-right text-xs tabular-nums text-muted-foreground group-hover:bg-muted/40 group-data-[state=selected]:bg-primary/10">
                      {index + 1}
                    </td>
                    {raw.map((cell, cellIndex) => {
                      const column = result.columns?.[cellIndex] ?? "";
                      const text = cellText(cell);
                      const nullish = isNullish(text);
                      return (
                        <td
                          key={cellIndex}
                          title={cellTitle(column, text)}
                          className={cn(
                            "truncate border-b border-border/50 px-3 align-middle leading-5",
                            contained ? "max-w-[14rem]" : "max-w-[28rem]",
                            nullish && "italic text-muted-foreground/50",
                          )}
                        >
                          {cellIndex === statusIndex ? (
                            <StatusBadge value={text} />
                          ) : (
                            displayCell(column, text)
                          )}
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
