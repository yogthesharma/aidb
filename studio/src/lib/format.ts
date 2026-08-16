export function isNullish(value: string): boolean {
  return value === "NULL" || value === "";
}

export function looksJson(value: string): boolean {
  const trimmed = value.trim();
  return (
    (trimmed.startsWith("{") && trimmed.endsWith("}")) ||
    (trimmed.startsWith("[") && trimmed.endsWith("]"))
  );
}

export function inferType(column: string, value: string): string {
  if (isNullish(value)) {
    return "null";
  }
  if (column.endsWith("_ms") || column.endsWith("_at")) {
    return "time";
  }
  if (column.includes("json") || looksJson(value)) {
    return "json";
  }
  if (column === "cost_usd" || column.includes("distance")) {
    return "real";
  }
  if (
    /(_id$|^id$|tokens|count|bytes|chunks|ordinal|seq|dimensions)/i.test(column) &&
    /^-?\d+(\.\d+)?$/.test(value)
  ) {
    return "int";
  }
  if (/^-?\d+$/.test(value)) {
    return "int";
  }
  if (/^-?\d+\.\d+$/.test(value)) {
    return "real";
  }
  return "text";
}

export function formatEpochMs(value: string): string | null {
  const n = Number(value);
  if (!Number.isFinite(n) || n < 1_000_000_000_000) {
    return null;
  }
  const date = new Date(n);
  if (Number.isNaN(date.getTime())) {
    return null;
  }
  return date.toISOString().replace("T", " ").slice(0, 19);
}

export function relativeMs(value: string): string | null {
  const n = Number(value);
  if (!Number.isFinite(n) || n < 1_000_000_000_000) {
    return null;
  }
  const delta = Date.now() - n;
  const abs = Math.abs(delta);
  const ago = delta >= 0;
  const unit =
    abs < 60_000
      ? `${Math.max(1, Math.round(abs / 1000))}s`
      : abs < 3_600_000
        ? `${Math.round(abs / 60_000)}m`
        : abs < 86_400_000
          ? `${Math.round(abs / 3_600_000)}h`
          : `${Math.round(abs / 86_400_000)}d`;
  return ago ? `${unit} ago` : `in ${unit}`;
}

export function prettyJson(value: string): string {
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}

export function displayCell(column: string, value: string): string {
  if (isNullish(value)) {
    return "null";
  }
  if (column.endsWith("_ms")) {
    return formatEpochMs(value) ?? value;
  }
  if (looksJson(value) && value.length > 80) {
    return `${value.slice(0, 77)}…`;
  }
  return value;
}

export function cellTitle(column: string, value: string): string {
  if (column.endsWith("_ms")) {
    const rel = relativeMs(value);
    return rel ? `${value} · ${rel}` : value;
  }
  return value;
}

export function formatMs(ms: number): string {
  if (ms < 1) {
    return "<1ms";
  }
  if (ms < 1000) {
    return `${Math.round(ms)}ms`;
  }
  return `${(ms / 1000).toFixed(2)}s`;
}

export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined) {
    return "—";
  }
  return n.toLocaleString();
}
