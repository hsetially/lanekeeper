import type { ComponentProps } from "react";
import { Popover as PopoverPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";
import { useOverlayContainer } from "./overlay-root";

export function Popover(props: ComponentProps<typeof PopoverPrimitive.Root>) {
  return <PopoverPrimitive.Root data-slot="popover" {...props} />;
}

export function PopoverTrigger(props: ComponentProps<typeof PopoverPrimitive.Trigger>) {
  return <PopoverPrimitive.Trigger data-slot="popover-trigger" {...props} />;
}

export function PopoverAnchor(props: ComponentProps<typeof PopoverPrimitive.Anchor>) {
  return <PopoverPrimitive.Anchor data-slot="popover-anchor" {...props} />;
}

/** A popover's content is a `role="dialog"`, and a dialog must have a name (axe `aria-dialog-name`): so one of the two is required. */
type Named = { "aria-label": string; "aria-labelledby"?: never } | { "aria-labelledby": string; "aria-label"?: never };

export function PopoverContent({ className, align = "start", sideOffset = 4, ...props }: Omit<ComponentProps<typeof PopoverPrimitive.Content>, "aria-label" | "aria-labelledby"> & Named) {
  const container = useOverlayContainer();
  return (
    <PopoverPrimitive.Portal container={container}>
      <PopoverPrimitive.Content
        data-slot="popover-content"
        align={align}
        sideOffset={sideOffset}
        className={cn("z-40 w-72 rounded-md border border-border bg-popover p-3 text-popover-foreground shadow-popover animate-lk-pop-in", className)}
        {...props}
      />
    </PopoverPrimitive.Portal>
  );
}
