import type { ComponentProps } from "react";
import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui";
import { Check } from "@/icons";
import { cn } from "@/lib/utils";
import { useOverlayContainer } from "./overlay-root";

// Not modal by default. A modal menu marks the rest of the page aria-hidden while the page's controls stay in the tab order,
// which is what axe's `aria-hidden-focus` rule reports; the WAI-ARIA menu button pattern has no such trap (Escape closes the
// menu, a click outside closes it, and focus goes back to the button). A caller that really wants a modal menu passes `modal`.
export function DropdownMenu({ modal = false, ...props }: ComponentProps<typeof DropdownMenuPrimitive.Root>) {
  return <DropdownMenuPrimitive.Root data-slot="dropdown-menu" modal={modal} {...props} />;
}

export function DropdownMenuTrigger(props: ComponentProps<typeof DropdownMenuPrimitive.Trigger>) {
  return <DropdownMenuPrimitive.Trigger data-slot="dropdown-menu-trigger" {...props} />;
}

export function DropdownMenuGroup(props: ComponentProps<typeof DropdownMenuPrimitive.Group>) {
  return <DropdownMenuPrimitive.Group data-slot="dropdown-menu-group" {...props} />;
}

export function DropdownMenuContent({ className, sideOffset = 4, align = "end", ...props }: ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  const container = useOverlayContainer();
  return (
    <DropdownMenuPrimitive.Portal container={container}>
      <DropdownMenuPrimitive.Content
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        align={align}
        className={cn("z-40 min-w-48 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-popover animate-lk-pop-in", className)}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

// Menu items are 32px high.
const itemClasses =
  "relative flex h-8 cursor-pointer select-none items-center gap-2 rounded-sm px-2 text-sm text-popover-foreground outline-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-accent [&_svg]:size-3.5 [&_svg]:shrink-0";

export function DropdownMenuItem({ className, destructive = false, ...props }: ComponentProps<typeof DropdownMenuPrimitive.Item> & { destructive?: boolean }) {
  return <DropdownMenuPrimitive.Item data-slot="dropdown-menu-item" className={cn(itemClasses, destructive && "text-danger-fg", className)} {...props} />;
}

export function DropdownMenuCheckboxItem({ className, children, ...props }: ComponentProps<typeof DropdownMenuPrimitive.CheckboxItem>) {
  return (
    <DropdownMenuPrimitive.CheckboxItem data-slot="dropdown-menu-checkbox-item" className={cn(itemClasses, "pl-7", className)} {...props}>
      <span className="absolute left-2 flex size-3.5 items-center justify-center">
        <DropdownMenuPrimitive.ItemIndicator>
          <Check aria-hidden="true" />
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.CheckboxItem>
  );
}

export function DropdownMenuRadioGroup(props: ComponentProps<typeof DropdownMenuPrimitive.RadioGroup>) {
  return <DropdownMenuPrimitive.RadioGroup data-slot="dropdown-menu-radio-group" {...props} />;
}

/** One choice of a group where exactly one is on (the theme: Light, Dark, System). The check marks the chosen one. */
export function DropdownMenuRadioItem({ className, children, ...props }: ComponentProps<typeof DropdownMenuPrimitive.RadioItem>) {
  return (
    <DropdownMenuPrimitive.RadioItem data-slot="dropdown-menu-radio-item" className={cn(itemClasses, "pl-7", className)} {...props}>
      <span className="absolute left-2 flex size-3.5 items-center justify-center">
        <DropdownMenuPrimitive.ItemIndicator>
          <Check aria-hidden="true" />
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.RadioItem>
  );
}

export function DropdownMenuLabel({ className, ...props }: ComponentProps<typeof DropdownMenuPrimitive.Label>) {
  return <DropdownMenuPrimitive.Label data-slot="dropdown-menu-label" className={cn("px-2 py-1.5 text-xs font-medium text-muted-foreground", className)} {...props} />;
}

export function DropdownMenuSeparator({ className, ...props }: ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return <DropdownMenuPrimitive.Separator data-slot="dropdown-menu-separator" className={cn("-mx-1 my-1 h-px bg-border", className)} {...props} />;
}

export function DropdownMenuShortcut({ className, ...props }: ComponentProps<"span">) {
  return <span data-slot="dropdown-menu-shortcut" className={cn("ml-auto font-mono text-xs text-muted-foreground", className)} {...props} />;
}
