export type PeekTarget =
  | { kind: "document"; id: string }
  | { kind: "run"; id: string }
  | { kind: "model"; name: string }
  | { kind: "table"; name: string }
  | { kind: "row"; title: string; fields: Record<string, string> };

export function inferPeek(row: Record<string, string>): PeekTarget {
  const documentId = row.document_id;
  if (documentId && documentId !== "NULL") {
    return { kind: "document", id: documentId };
  }
  const runId = row.run_id;
  if (runId && runId !== "NULL") {
    return { kind: "run", id: runId };
  }
  const experimentId = row.experiment_id;
  if (experimentId && experimentId !== "NULL") {
    return { kind: "run", id: experimentId };
  }
  const id = row.id;
  if (id && id !== "NULL" && id.startsWith("doc_")) {
    return { kind: "document", id };
  }
  if (id && id !== "NULL" && id.startsWith("run_")) {
    return { kind: "run", id };
  }
  if (row.type && row.name && row.name !== "NULL") {
    return { kind: "table", name: row.name };
  }
  if (row.provider && row.name && row.name !== "NULL") {
    return { kind: "model", name: row.name };
  }
  return {
    kind: "row",
    title: row.id ?? row.name ?? row.key ?? "Row",
    fields: row,
  };
}

export function peekKey(target: PeekTarget): string {
  switch (target.kind) {
    case "document":
      return `document:${target.id}`;
    case "run":
      return `run:${target.id}`;
    case "model":
      return `model:${target.name}`;
    case "table":
      return `table:${target.name}`;
    case "row":
      return `row:${target.title}`;
  }
}
