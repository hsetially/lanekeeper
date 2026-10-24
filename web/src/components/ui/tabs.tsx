import type { ComponentProps } from "react";
import { Tabs as TabsPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

export function Tabs({ className, ...props }: ComponentProps<typeof TabsPrimitive.Root>) {
  return <TabsPrimitive.Root data-slot="tabs" className={cn("flex flex-col", className)} {...props} />;
}

// Underline tabs: 36px high, a 2px bar in the primary colour on the selected one. Detail tabs are routes:
// render TabsTrigger asChild around a router link.
export function TabsList({ className, ...props }: ComponentProps<typeof TabsPrimitive.List>) {
  return <TabsPrimitive.List data-slot="tabs-list" className={cn("flex flex-wrap items-end gap-4 border-b border-border", className)} {...props} />;
}

export function TabsTrigger({ className, ...props }: ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        "-mb-px inline-flex h-9 cursor-pointer items-center gap-1.5 whitespace-nowrap border-0 border-b-2 border-transparent bg-transparent px-0.5 text-base font-medium text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 data-[state=active]:border-primary data-[state=active]:text-foreground",
        className,
      )}
      {...props}
    />
  );
}

export function TabsContent({ className, ...props }: ComponentProps<typeof TabsPrimitive.Content>) {
  return <TabsPrimitive.Content data-slot="tabs-content" className={cn("outline-offset-2", className)} {...props} />;
}

/** The count next to a tab label (Files 412, Findings 8). */
export function TabCount({ className, ...props }: ComponentProps<"span">) {
  return <span data-slot="tab-count" className={cn("text-xs font-normal tabular-nums text-muted-foreground", className)} {...props} />;
}
