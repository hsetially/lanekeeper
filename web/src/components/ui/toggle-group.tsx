import type { ComponentProps } from "react";
import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

// SegmentedControl: Diff view (Split, Unified), filters (Drift, All, Tenant), approval tabs.
// Single selection is exposed as a radio group by Radix, which is the right semantics for "one of".
export function ToggleGroup({ className, ...props }: ComponentProps<typeof ToggleGroupPrimitive.Root>) {
  return <ToggleGroupPrimitive.Root data-slot="toggle-group" className={cn("inline-flex items-center gap-0.5 rounded-sm border border-border bg-muted p-0.5", className)} {...props} />;
}

export function ToggleGroupItem({ className, ...props }: ComponentProps<typeof ToggleGroupPrimitive.Item>) {
  return (
    <ToggleGroupPrimitive.Item
      data-slot="toggle-group-item"
      className={cn(
        "inline-flex h-6 cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap rounded-sm border border-transparent bg-transparent px-2 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 data-[state=on]:border-border data-[state=on]:bg-background data-[state=on]:text-foreground [&_svg]:size-3.5",
        className,
      )}
      {...props}
    />
  );
}
