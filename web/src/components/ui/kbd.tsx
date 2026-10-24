import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

/** A key hint. Glyphs for ↑ ↓ ↵ ⌘ ⇧ are the only glyph "icons" the design allows. */
export function Kbd({ className, ...props }: ComponentProps<"kbd">) {
  return (
    <kbd
      data-slot="kbd"
      className={cn("inline-flex min-h-[18px] min-w-[18px] items-center justify-center rounded-sm border border-b-2 border-border bg-muted px-1 font-mono text-xs text-muted-foreground", className)}
      {...props}
    />
  );
}
