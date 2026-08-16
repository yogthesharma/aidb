import { PAGE_SEGMENT as SEGMENTS } from "@/lib/catalog.mjs";
import type { PeekTarget } from "@/lib/peek";

export type StudioPage = keyof typeof SEGMENTS;

export const PAGE_SEGMENT: Record<StudioPage, string> = SEGMENTS;

const SEGMENT_PAGE: Record<string, StudioPage> = Object.fromEntries(
  Object.entries(SEGMENTS).map(([page, segment]) => [segment, page]),
) as Record<string, StudioPage>;

export type StudioLocationState = {
  rowPeek?: PeekTarget;
};

export function pageHref(page: StudioPage): string {
  return `/${PAGE_SEGMENT[page]}`;
}

export function peekHref(target: PeekTarget): string {
  switch (target.kind) {
    case "document":
      return `/documents/${encodeURIComponent(target.id)}`;
    case "run":
      return `/runs/${encodeURIComponent(target.id)}`;
    case "model":
      return `/models/${encodeURIComponent(target.name)}`;
    case "table":
      return `/file/${encodeURIComponent(target.name)}`;
    case "row":
      return "/sql";
  }
}

export function parseStudioPath(pathname: string): {
  known: boolean;
  page: StudioPage;
  peek: PeekTarget | null;
} {
  const parts = pathname.split("/").filter(Boolean).map((part) => decodeURIComponent(part));
  if (parts.length === 0) {
    return { known: false, page: "overview", peek: null };
  }
  const page = SEGMENT_PAGE[parts[0] ?? ""];
  if (!page) {
    return { known: false, page: "overview", peek: null };
  }
  if (parts.length === 1) {
    return { known: true, page, peek: null };
  }
  if (parts.length !== 2) {
    return { known: false, page, peek: null };
  }
  const id = parts[1] ?? "";
  if (page === "overview") {
    return { known: true, page, peek: { kind: "table", name: id } };
  }
  if (page === "documents") {
    return { known: true, page, peek: { kind: "document", id } };
  }
  if (page === "runs") {
    return { known: true, page, peek: { kind: "run", id } };
  }
  if (page === "models") {
    return { known: true, page, peek: { kind: "model", name: id } };
  }
  if (page === "experiments") {
    return { known: true, page, peek: { kind: "run", id } };
  }
  return { known: false, page, peek: null };
}
