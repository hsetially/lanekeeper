import type { ReactNode, RefObject } from "react";
import { SafeText } from "@/components/app/SafeText";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { CircleCheck, Hourglass, Info, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";

// What a write that went through tells the person, inside the dialog that made it (H-3): the title says it in words, the icon and
// the colour say its tone, and nothing here is anything but what the server answered. It is a dialog the person reads, so focus
// starts on the dialog itself (as in ProblemDialog) and Tab goes to its content and then to Close.

type Tone = "success" | "warning" | "info" | "approval";

const ICON = { success: CircleCheck, warning: TriangleAlert, info: Info, approval: Hourglass } as const;
const COLOUR: Record<Tone, string> = { success: "text-success-fg", warning: "text-warning-fg", info: "text-info-fg", approval: "text-status-approval-fg" };

function focusDialog(event: Event): void {
  if (event.target instanceof HTMLElement) {
    event.preventDefault();
    event.target.focus();
  }
}

type Props = {
  open: boolean;
  onClose: () => void;
  tone: Tone;
  title: string;
  /** What happened, in one or two sentences. */
  description: string;
  children?: ReactNode;
  /** The screen's own buttons (Retry), placed before Close. */
  actions?: ReactNode;
  /** Where focus goes when the dialog closes: the button that started the flow. Without it, the control that had focus when this dialog opened. */
  returnFocusRef?: RefObject<HTMLElement | null>;
};

export function OutcomeDialog({ open, onClose, tone, title, description, children, actions, returnFocusRef }: Props) {
  const Icon = ICON[tone];
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <DialogContent size="lg" showCloseButton={false} onOpenAutoFocus={focusDialog} className="focus-visible:outline-none" {...(returnFocusRef ? { returnFocusRef } : {})}>
        <DialogHeader>
          <DialogTitle data-tone={tone} className="flex items-center gap-2">
            <Icon aria-hidden="true" className={cn("size-4 shrink-0", COLOUR[tone])} />
            <SafeText text={title} />
          </DialogTitle>
          <DialogDescription>
            <SafeText text={description} />
          </DialogDescription>
        </DialogHeader>
        {children}
        <DialogFooter>
          {actions}
          <DialogClose asChild>
            <Button variant="secondary">Close</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
