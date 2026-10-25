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

/**
 * Where focus is when the content has gone: nowhere (the body), because the control inside it was removed. Anything else was put there
 * after the popover closed.
 */
function focusIsLost(): boolean {
  const active = document.activeElement;
  return active === null || active === document.body || active === document.documentElement;
}

export function PopoverContent({ className, align = "start", sideOffset = 4, onCloseAutoFocus, ...props }: Omit<ComponentProps<typeof PopoverPrimitive.Content>, "aria-label" | "aria-labelledby"> & Named) {
  const container = useOverlayContainer();
  return (
    <PopoverPrimitive.Portal container={container}>
      <PopoverPrimitive.Content
        data-slot="popover-content"
        align={align}
        sideOffset={sideOffset}
        className={cn("z-40 w-72 rounded-md border border-border bg-popover p-3 text-popover-foreground shadow-popover animate-lk-pop-in", className)}
        // Radix gives focus back to the trigger in a timer that it sets when the content goes, so there is a task in which the popover is
        // closed and focus has not come back yet. A control that took focus in that task (the next chip, a field, the popover opened in
        // between: a key from a switch device, a voice command or a script does not wait for it) keeps it: the late return would take focus
        // away from it, and focus leaving a popover that is open dismisses that one too. Radix does the same for a click outside.
        onCloseAutoFocus={(event) => {
          onCloseAutoFocus?.(event);
          if (!event.defaultPrevented && !focusIsLost()) event.preventDefault();
        }}
        {...props}
      />
    </PopoverPrimitive.Portal>
  );
}
