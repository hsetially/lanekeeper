import type { ComponentProps } from "react";
import { Command as CommandPrimitive } from "cmdk";
import { Search } from "@/icons";
import { cn } from "@/lib/utils";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./dialog";
import { Kbd } from "./kbd";

export function Command({ className, ...props }: ComponentProps<typeof CommandPrimitive>) {
  return <CommandPrimitive data-slot="command" className={cn("flex h-full w-full flex-col overflow-hidden rounded-md bg-popover text-popover-foreground", className)} {...props} />;
}

type CommandDialogProps = ComponentProps<typeof Dialog> & {
  title: string;
  description: string;
  commandProps?: ComponentProps<typeof CommandPrimitive>;
};

/** The palette: a 640px dialog around Command. The title and description are for assistive technology only. */
export function CommandDialog({ title, description, children, commandProps, ...props }: CommandDialogProps) {
  return (
    <Dialog {...props}>
      <DialogContent size="palette" showCloseButton={false} className="gap-0 overflow-hidden p-0">
        <DialogTitle className="sr-only">{title}</DialogTitle>
        <DialogDescription className="sr-only">{description}</DialogDescription>
        <Command label={title} {...commandProps}>
          {children}
        </Command>
      </DialogContent>
    </Dialog>
  );
}

// The input is 44px high, as in the handoff palette.
export function CommandInput({ className, ...props }: ComponentProps<typeof CommandPrimitive.Input>) {
  return (
    <div data-slot="command-input-wrapper" className="flex h-11 items-center gap-2 border-b border-border px-3">
      <Search aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
      <CommandPrimitive.Input
        data-slot="command-input"
        className={cn("h-full min-w-0 flex-1 bg-transparent text-base text-foreground outline-none placeholder:text-muted-foreground focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50", className)}
        {...props}
      />
    </div>
  );
}

export function CommandList({ className, ...props }: ComponentProps<typeof CommandPrimitive.List>) {
  return <CommandPrimitive.List data-slot="command-list" className={cn("max-h-[360px] overflow-y-auto overflow-x-hidden p-1", className)} {...props} />;
}

export function CommandEmpty({ className, ...props }: ComponentProps<typeof CommandPrimitive.Empty>) {
  return <CommandPrimitive.Empty data-slot="command-empty" className={cn("px-3 py-6 text-center text-sm text-muted-foreground", className)} {...props} />;
}

export function CommandGroup({ className, ...props }: ComponentProps<typeof CommandPrimitive.Group>) {
  return (
    <CommandPrimitive.Group
      data-slot="command-group"
      className={cn("overflow-hidden text-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-muted-foreground", className)}
      {...props}
    />
  );
}

export function CommandSeparator({ className, ...props }: ComponentProps<typeof CommandPrimitive.Separator>) {
  return <CommandPrimitive.Separator data-slot="command-separator" className={cn("-mx-1 my-1 h-px bg-border", className)} {...props} />;
}

// Items are 32px high; the active one takes the accent surface.
export function CommandItem({ className, ...props }: ComponentProps<typeof CommandPrimitive.Item>) {
  return (
    <CommandPrimitive.Item
      data-slot="command-item"
      className={cn("relative flex h-8 cursor-pointer select-none items-center gap-2 rounded-sm px-2 text-sm outline-none data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50 data-[selected=true]:bg-accent [&_svg]:size-3.5 [&_svg]:shrink-0", className)}
      {...props}
    />
  );
}

export function CommandShortcut({ className, ...props }: ComponentProps<typeof Kbd>) {
  return <Kbd data-slot="command-shortcut" className={cn("ml-auto", className)} {...props} />;
}
