import { Link } from "react-router";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { PAGE_COPY } from "@/components/app-sidebar";
import type { PeekTarget } from "@/lib/peek";
import { pageHref, type StudioPage } from "@/lib/studio-path";

function peekSection(peek: PeekTarget): { page: StudioPage; label: string; leaf: string } {
  switch (peek.kind) {
    case "document":
      return { page: "documents", label: "Documents", leaf: peek.id };
    case "run":
      return { page: "runs", label: "Runs", leaf: peek.id };
    case "model":
      return { page: "models", label: "Models", leaf: peek.name };
    case "table":
      return { page: "overview", label: "File", leaf: peek.name };
    case "row":
      return { page: "sql", label: "SQL", leaf: peek.title };
  }
}

export function StudioBreadcrumb({
  page,
  peek,
}: {
  page: StudioPage;
  peek: PeekTarget | null;
}) {
  const section = peek ? peekSection(peek) : null;
  const pageLabel = PAGE_COPY[page].title;
  const midLabel = section?.label ?? pageLabel;
  const midPage = section?.page ?? page;
  const leaf = section?.leaf;

  return (
    <Breadcrumb className="min-w-0">
      <BreadcrumbList className="flex-nowrap">
        <BreadcrumbItem>
          <BreadcrumbLink asChild>
            <Link to="/file">app.db</Link>
          </BreadcrumbLink>
        </BreadcrumbItem>
        <BreadcrumbSeparator />
        <BreadcrumbItem className="min-w-0">
          {leaf ? (
            <BreadcrumbLink asChild>
              <Link to={pageHref(midPage)}>{midLabel}</Link>
            </BreadcrumbLink>
          ) : (
            <BreadcrumbPage>{pageLabel}</BreadcrumbPage>
          )}
        </BreadcrumbItem>
        {leaf ? (
          <>
            <BreadcrumbSeparator />
            <BreadcrumbItem className="min-w-0">
              <BreadcrumbPage className="max-w-[16rem] truncate font-medium" title={leaf}>
                {leaf}
              </BreadcrumbPage>
            </BreadcrumbItem>
          </>
        ) : null}
      </BreadcrumbList>
    </Breadcrumb>
  );
}
