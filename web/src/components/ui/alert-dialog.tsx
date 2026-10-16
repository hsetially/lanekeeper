import type { ComponentProps } from "react";
import { AlertDialog as AlertDialogPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";
import { buttonVariants } from "./button";
import { dialogSizes } from "./dialog";

export function AlertDialog(props: ComponentProps<typeof AlertDialogPrimitive.Root>) {
  return <AlertDialogPrimitive.Root data-slot="alert-dialog" {...props} />;
}

export function AlertDialogTrigger(props: ComponentProps<typeof AlertDialogPrimitive.Trigger>) {
  return <AlertDialogPrimitive.Trigger data-slot="alert-dialog-trigger" {...props} />;
}

// Radix puts initial focus on the Cancel control, the least destructive one, as the handoff asks.
export function AlertDialogContent({ className, size = "md", ...props }: ComponentProps<typeof AlertDialogPrimitive.Content> & { size?: keyof typeof dialogSizes }) {
  return (
    <AlertDialogPrimitive.Portal>
      <AlertDialogPrimitive.Overlay data-slot="alert-dialog-overlay" className="fixed inset-0 grid place-items-center overflow-y-auto bg-overlay p-4 animate-lk-fade-in">
        <AlertDialogPrimitive.Content
          data-slot="alert-dialog-content"
          className={cn("relative flex w-full flex-col gap-3 rounded-md border border-border bg-popover p-4 text-popover-foreground shadow-popover animate-lk-pop-in", dialogSizes[size], className)}
          {...props}
        />
      </AlertDialogPrimitive.Overlay>
    </AlertDialogPrimitive.Portal>
  );
}

export function AlertDialogHeader({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="alert-dialog-header" className={cn("flex flex-col gap-1", className)} {...props} />;
}

export function AlertDialogFooter({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="alert-dialog-footer" className={cn("flex items-center justify-end gap-2", className)} {...props} />;
}

export function AlertDialogTitle({ className, ...props }: ComponentProps<typeof AlertDialogPrimitive.Title>) {
  return <AlertDialogPrimitive.Title data-slot="alert-dialog-title" className={cn("text-lg font-semibold text-foreground", className)} {...props} />;
}

export function AlertDialogDescription({ className, ...props }: ComponentProps<typeof AlertDialogPrimitive.Description>) {
  return <AlertDialogPrimitive.Description data-slot="alert-dialog-description" className={cn("text-sm text-muted-foreground", className)} {...props} />;
}

/** Confirms and closes. Call event.preventDefault() in onClick to keep the dialog open while the write runs. */
export function AlertDialogAction({ className, variant = "default", ...props }: ComponentProps<typeof AlertDialogPrimitive.Action> & { variant?: "default" | "destructive" }) {
  return <AlertDialogPrimitive.Action data-slot="alert-dialog-action" className={cn(buttonVariants({ variant }), className)} {...props} />;
}

export function AlertDialogCancel({ className, ...props }: ComponentProps<typeof AlertDialogPrimitive.Cancel>) {
  return <AlertDialogPrimitive.Cancel data-slot="alert-dialog-cancel" className={cn(buttonVariants({ variant: "ghost" }), className)} {...props} />;
}
