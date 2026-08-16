import { useRef, type UIEvent } from "react";
import { highlightSql } from "@/lib/highlight";
import { cn } from "@/lib/utils";

export function SqlEditor({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  const preRef = useRef<HTMLPreElement>(null);
  const areaRef = useRef<HTMLTextAreaElement>(null);

  function syncScroll(event: UIEvent<HTMLTextAreaElement>) {
    const pre = preRef.current;
    if (!pre) {
      return;
    }
    pre.scrollTop = event.currentTarget.scrollTop;
    pre.scrollLeft = event.currentTarget.scrollLeft;
  }

  return (
    <div className="relative min-h-0 flex-1 overflow-hidden">
      <pre
        ref={preRef}
        aria-hidden
        className={cn(
          "pointer-events-none absolute inset-0 m-0 overflow-auto whitespace-pre-wrap break-words px-3 py-2.5 text-base leading-6",
          "font-sans",
        )}
        dangerouslySetInnerHTML={{ __html: `${highlightSql(value)}\n` }}
      />
      <textarea
        ref={areaRef}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onScroll={syncScroll}
        spellCheck={false}
          className={cn(
          "absolute inset-0 z-10 m-0 h-full w-full resize-none overflow-auto bg-transparent px-3 py-2.5 text-base leading-6 text-transparent caret-foreground outline-none selection:bg-primary/25",
          "font-sans",
        )}
      />
    </div>
  );
}
