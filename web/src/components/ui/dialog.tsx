import type { ComponentProps } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { X } from "@/icons";
import { cn } from "@/lib/utils";
import { Button } from "./button";
import { useReturnFocus, type ReturnFocusProps } from "./return-focus";

export function Dialog(props: ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

export function DialogTrigger(props: ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />;
}

export function DialogClose(props: ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />;
}

export const dialogSizes = {
  sm: "max-w-[400px]",
  md: "max-w-[480px]",
  lg: "max-w-[600px]",
  xl: "max-w-[760px]", // the shortcuts sheet
  palette: "max-w-[640px]", // the command palette
} as const;

type DialogContentProps = ComponentProps<typeof DialogPrimitive.Content> &
  ReturnFocusProps & {
    size?: keyof typeof dialogSizes;
    showCloseButton?: boolean;
  };

// The content sits inside the overlay and is centred by the overlay's grid, so no transform is needed to centre it
// and the open animation does not fight one. A click on the overlay is a click outside the content.
// Always give the dialog a DialogTitle (hidden with sr-only if the design shows none) and a description. On close, focus goes back
// to the control that opened the dialog (src/components/ui/return-focus.ts), or to `returnFocusRef`.
export function DialogContent({ className, children, size = "md", showCloseButton = true, returnFocusRef, onOpenAutoFocus, onCloseAutoFocus, ...props }: DialogContentProps) {
  const focus = useReturnFocus({ returnFocusRef, onOpenAutoFocus, onCloseAutoFocus });
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay data-slot="dialog-overlay" className="fixed inset-0 grid place-items-center overflow-y-auto bg-overlay p-4 animate-lk-fade-in">
        <DialogPrimitive.Content
          data-slot="dialog-content"
          className={cn("relative flex w-full flex-col gap-3 rounded-md border border-border bg-popover p-4 text-popover-foreground shadow-popover animate-lk-pop-in", dialogSizes[size], className)}
          {...props}
          onOpenAutoFocus={focus.onOpenAutoFocus}
          onCloseAutoFocus={focus.onCloseAutoFocus}
        >
          {children}
          {showCloseButton ? (
            <DialogPrimitive.Close asChild>
              <Button variant="ghost" size="icon-sm" aria-label="Close" className="absolute top-3 right-3">
                <X />
              </Button>
            </DialogPrimitive.Close>
          ) : null}
        </DialogPrimitive.Content>
      </DialogPrimitive.Overlay>
    </DialogPrimitive.Portal>
  );
}

export function DialogHeader({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="dialog-header" className={cn("flex flex-col gap-1 pr-8", className)} {...props} />;
}

export function DialogFooter({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="dialog-footer" className={cn("flex items-center justify-end gap-2", className)} {...props} />;
}

export function DialogTitle({ className, ...props }: ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title data-slot="dialog-title" className={cn("text-lg font-semibold text-foreground", className)} {...props} />;
}

export function DialogDescription({ className, ...props }: ComponentProps<typeof DialogPrimitive.Description>) {
  return <DialogPrimitive.Description data-slot="dialog-description" className={cn("text-sm text-muted-foreground", className)} {...props} />;
}
