import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

export function StatusBadge({ value }: { value: string }) {
  const tone =
    value === "succeeded" || value === "ready" || value === "ok"
      ? "secondary"
      : value === "failed" || value === "cancelled"
        ? "destructive"
        : value === "awaiting_approval" || value === "pending" || value === "indexing"
          ? "outline"
          : "outline";
  return (
    <Badge
      variant={tone}
      className={cn(
        "h-5 rounded-md px-1.5 text-xs font-normal leading-none",
        (value === "awaiting_approval" || value === "pending") &&
          "border-amber-500/40 text-amber-400",
        value === "running" && "border-sky-500/40 text-sky-400",
      )}
    >
      {value}
    </Badge>
  );
}
