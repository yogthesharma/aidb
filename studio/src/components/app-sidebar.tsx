import {
  Bot,
  Database,
  FileText,
  FlaskConical,
  LayoutDashboard,
  Search,
  SquareTerminal,
  Table2,
  Workflow,
} from "lucide-react";
import { Link, NavLink } from "react-router";
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from "@/components/ui/sidebar";
import { pageHref, peekHref, type StudioPage } from "@/lib/studio-path";

export type { StudioPage };

const NAV: { id: StudioPage; label: string; icon: typeof Database }[] = [
  { id: "overview", label: "File", icon: LayoutDashboard },
  { id: "sql", label: "SQL", icon: SquareTerminal },
  { id: "documents", label: "Documents", icon: FileText },
  { id: "search", label: "Search", icon: Search },
  { id: "runs", label: "Runs", icon: Workflow },
  { id: "experiments", label: "Experiments", icon: FlaskConical },
  { id: "models", label: "Models", icon: Bot },
];

export function AppSidebar({
  page,
  waiting,
  documents,
  experiments,
  tables,
  tableName,
}: {
  page: StudioPage;
  waiting?: number | null;
  documents?: number | null;
  experiments?: number | null;
  tables: { name: string; type: string }[];
  tableName?: string | null;
}) {
  return (
    <Sidebar collapsible="icon" className="border-r">
      <SidebarHeader className="flex h-12 justify-center border-b p-0 px-3">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton tooltip="AIDB Studio" asChild>
              <Link to="/file">
                <div className="flex size-6 items-center justify-center rounded-md bg-sidebar-primary text-sidebar-primary-foreground">
                  <Database className="size-3.5" />
                </div>
                <div className="grid flex-1 text-left leading-tight">
                  <span className="truncate text-sm font-medium">AIDB</span>
                  <span className="truncate text-xs text-muted-foreground">
                    app.db
                  </span>
                </div>
              </Link>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupLabel>Browse</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {NAV.map((item) => (
                <SidebarMenuItem key={item.id}>
                  <SidebarMenuButton
                    asChild
                    isActive={page === item.id}
                    tooltip={item.label}
                    className={
                      item.id === "runs" ||
                      item.id === "documents" ||
                      item.id === "experiments"
                        ? "pr-8"
                        : undefined
                    }
                  >
                    <NavLink to={pageHref(item.id)}>
                      <item.icon />
                      <span>{item.label}</span>
                    </NavLink>
                  </SidebarMenuButton>
                  {item.id === "runs" && waiting ? (
                    <SidebarMenuBadge>{waiting}</SidebarMenuBadge>
                  ) : null}
                  {item.id === "documents" && documents ? (
                    <SidebarMenuBadge>{documents}</SidebarMenuBadge>
                  ) : null}
                  {item.id === "experiments" && experiments ? (
                    <SidebarMenuBadge>{experiments}</SidebarMenuBadge>
                  ) : null}
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
        {tables?.length ? (
          <SidebarGroup>
            <SidebarGroupLabel>Tables</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                {tables.map((table) => (
                  <SidebarMenuItem key={table.name}>
                    <SidebarMenuButton
                      asChild
                      tooltip={`${table.type} ${table.name}`}
                      isActive={tableName === table.name}
                      className="font-normal"
                    >
                      <NavLink to={peekHref({ kind: "table", name: table.name })}>
                        <Table2 />
                        <span>{table.name}</span>
                      </NavLink>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        ) : null}
      </SidebarContent>
      <SidebarRail />
    </Sidebar>
  );
}

export const PAGE_COPY: Record<
  StudioPage,
  { title: string; description: string }
> = {
  overview: {
    title: "File",
    description: "aidb_meta and objects in this SQLite file",
  },
  sql: {
    title: "SQL",
    description: "POST /sql on the same process",
  },
  documents: {
    title: "Documents",
    description: "index_status is a column, not a queue",
  },
  search: {
    title: "Search",
    description: "aidb_search — vec, FTS, or hybrid",
  },
  runs: {
    title: "Runs",
    description: "Durable execution. Resume is SQL.",
  },
  experiments: {
    title: "Experiments",
    description: "experiment_results — a view over runs",
  },
  models: {
    title: "Models",
    description: "key_name only — never the secret",
  },
};
