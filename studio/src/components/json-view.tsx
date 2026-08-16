import { highlightJson } from "@/lib/highlight";
import { prettyJson } from "@/lib/format";
import { cn } from "@/lib/utils";

export function JsonView({
  value,
  className,
}: {
  value: string;
  className?: string;
}) {
  return (
    <pre
      className={cn(
        "max-h-48 max-w-full overflow-auto whitespace-pre-wrap break-all text-sm leading-5",
        className,
      )}
      dangerouslySetInnerHTML={{ __html: highlightJson(prettyJson(value)) }}
    />
  );
}
