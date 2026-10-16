import type { ComponentProps } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";
import { cn } from "@/lib/utils";

// Height 20, 4px radius, icon 12. Status badges build on this with the status tokens (app/StatusBadge).
export const badgeBase = "inline-flex h-5 shrink-0 items-center gap-1 whitespace-nowrap rounded-sm border px-1.5 text-xs font-medium [&_svg]:size-3 [&_svg]:shrink-0";

export const badgeVariants = cva(badgeBase, {
  variants: {
    variant: {
      outline: "border-border bg-transparent text-foreground",
      muted: "border-border bg-muted text-muted-foreground",
      // The Admin-only badge uses the outline form of violet (the approval colours).
      approval: "border-status-approval-border bg-status-approval-bg text-status-approval-fg",
    },
  },
  defaultVariants: { variant: "outline" },
});

type BadgeProps = ComponentProps<"span"> & VariantProps<typeof badgeVariants> & { asChild?: boolean };

export function Badge({ className, variant = "outline", asChild = false, ...props }: BadgeProps) {
  const Comp = asChild ? Slot.Root : "span";
  return <Comp data-slot="badge" data-variant={variant} className={cn(badgeVariants({ variant }), className)} {...props} />;
}
