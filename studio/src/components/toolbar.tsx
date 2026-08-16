import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export function Toolbar({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex h-10 shrink-0 items-center gap-2 border-b px-3",
        "[&_[data-slot=input]]:h-8 [&_[data-slot=input]]:text-sm",
        "[&_[data-slot=select-trigger]]:h-8 [&_[data-slot=select-trigger]]:rounded-lg",
        "[&_[data-slot=button][data-size=sm]]:h-8 [&_[data-slot=button][data-size=sm]]:rounded-lg [&_[data-slot=button][data-size=sm]]:text-sm",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function VRule({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={cn("mx-0.5 h-3.5 w-px shrink-0 bg-border", className)}
    />
  );
}
