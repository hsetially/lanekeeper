import type { ComponentProps } from "react";
import { Tooltip as TooltipPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

export function TooltipProvider({ delayDuration = 500, ...props }: ComponentProps<typeof TooltipPrimitive.Provider>) {
  return <TooltipPrimitive.Provider delayDuration={delayDuration} {...props} />;
}

// Each Tooltip brings its own provider so it works anywhere; mount one TooltipProvider at the app root to share
// the open delay between neighbours. Opens after 500 ms on hover and at once on keyboard focus; Esc closes.
export function Tooltip({ delayDuration = 500, ...props }: ComponentProps<typeof TooltipPrimitive.Root>) {
  return (
    <TooltipPrimitive.Provider delayDuration={delayDuration}>
      <TooltipPrimitive.Root data-slot="tooltip" {...props} />
    </TooltipPrimitive.Provider>
  );
}

export function TooltipTrigger(props: ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />;
}

// The handoff tooltip is inverted: foreground background, background text, 12px, 24px high, 4px radius.
export function TooltipContent({ className, sideOffset = 4, children, ...props }: ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        className={cn("inline-flex min-h-6 max-w-80 items-center gap-2 rounded-sm bg-foreground px-2 py-1 text-xs text-background", className)}
        {...props}
      >
        {children}
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  );
}
