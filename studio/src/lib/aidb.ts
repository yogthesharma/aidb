import { authHeaders } from "@/lib/auth";
import { sqlString as quote } from "@/lib/catalog.mjs";

export type SqlResult = {
  ok: boolean;
  error?: string;
  columns?: string[];
  rows?: unknown[][];
  changed?: number;
};

export type Reach = "ok" | "down" | "unauthorized";

export async function probe(): Promise<Reach> {
  try {
    const response = await fetch("/health", { headers: authHeaders() });
    if (response.status === 401) {
      return "unauthorized";
    }
    if (!response.ok) {
      return "down";
    }
    const body: { ok?: boolean } = await response.json();
    return body.ok === true ? "ok" : "down";
  } catch {
    return "down";
  }
}

export async function runSqlTimed(
  sql: string,
): Promise<{ result: SqlResult; ms: number }> {
  const started = performance.now();
  const result = await runSql(sql);
  return { result, ms: Math.max(0, Math.round(performance.now() - started)) };
}

export async function runSql(sql: string): Promise<SqlResult> {
  const response = await fetch("/sql", {
    method: "POST",
    headers: {
      "content-type": "text/plain; charset=utf-8",
      ...authHeaders(),
    },
    body: sql,
  });
  if (response.status === 401) {
    return { ok: false, error: "bearer required" };
  }
  const body = (await response.json()) as SqlResult;
  if (!response.ok || body.ok === false) {
    return {
      ok: false,
      error: body.error ?? `HTTP ${response.status}`,
    };
  }
  return body;
}

export function cellText(value: unknown): string {
  if (value === null || value === undefined) {
    return "NULL";
  }
  if (typeof value === "object") {
    return JSON.stringify(value);
  }
  return String(value);
}

export const sqlString = quote;

export function metaMap(result: SqlResult | null): Map<string, string> {
  const map = new Map<string, string>();
  if (!result?.ok || !result.rows) {
    return map;
  }
  for (const row of result.rows) {
    map.set(cellText(row[0]), cellText(row[1]));
  }
  return map;
}

export function firstNumber(result: SqlResult | null): number | null {
  if (!result?.ok || !result.rows?.[0]) {
    return null;
  }
  const raw = result.rows[0][0];
  if (raw === null || raw === undefined) {
    return null;
  }
  const n = typeof raw === "number" ? raw : Number(raw);
  return Number.isFinite(n) ? n : null;
}

export function rowRecord(
  columns: string[],
  row: unknown[],
): Record<string, string> {
  const out: Record<string, string> = {};
  columns.forEach((column, index) => {
    out[column] = cellText(row[index]);
  });
  return out;
}

export function sqlIdent(name: string): string {
  if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
    return name;
  }
  return `"${name.replaceAll('"', '""')}"`;
}

export async function copyText(value: string): Promise<void> {
  await navigator.clipboard.writeText(value);
}
