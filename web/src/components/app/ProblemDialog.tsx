import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Hourglass, Lock, TriangleAlert } from "@/icons";
import { RequestId } from "./RequestId";
import { SafeText } from "./SafeText";

// The three answers to a write that is not a plain success: 409 Conflict (someone else changed it first), 423 Locked
// (the target is locked, say why and who holds it) and "sent for approval" (200 with outcome proposal_created, not
// applied until an Admin approves). Presentational: T1's problem parser and the write screens feed it.
type Variant = "conflict" | "locked" | "approval";

const DEFAULTS = {
  conflict: {
    title: "Someone else changed it first",
    description: "Nothing was applied. Review what changed, then re-apply your change on top.",
    Icon: TriangleAlert,
  },
  locked: {
    title: "Editing is locked",
    description: "The target is locked, so nothing was applied.",
    Icon: Lock,
  },
  approval: {
    title: "Sent for approval",
    description: "Your change is not applied until an Admin approves it.",
    Icon: Hourglass,
  },
} as const;

/**
 * Focus starts on the dialog itself. The first control in it is the request ID's copy button, and a keyboard hint that opens on a
 * button nobody tabbed to takes the first Escape; the footer's first action may be "Discard my changes", which is not a place to
 * start. The dialog is read first (its title and description name it), and Tab goes to the request ID, then the actions.
 */
function focusDialog(event: Event): void {
  if (event.target instanceof HTMLElement) {
    event.preventDefault();
    event.target.focus();
  }
}

type ProblemDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  variant: Variant;
  /** Replaces the default title. */
  title?: string;
  /** Replaces the default description: what happened, why, and the next step. */
  description?: string;
  requestId?: string;
  /** Details: the diff of what changed, who holds the lock, the link to the proposal. */
  children?: ReactNode;
  /** Footer buttons. Defaults to a single Close. */
  actions?: ReactNode;
  /** The corner cross, for actions that have no Close of their own. */
  showCloseButton?: boolean;
};

export function ProblemDialog({ open, onOpenChange, variant, title, description, requestId, children, actions, showCloseButton = false }: ProblemDialogProps) {
  const defaults = DEFAULTS[variant];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* The footer has the Close button (the default one, or the caller's own with its other actions); a corner cross beside it would be
          a second "Close" for a screen reader. A caller whose actions have no Close can ask for the cross. */}
      <DialogContent size="lg" showCloseButton={showCloseButton} onOpenAutoFocus={focusDialog} className="focus-visible:outline-none">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <defaults.Icon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
            <SafeText text={title ?? defaults.title} />
          </DialogTitle>
          <DialogDescription>
            <SafeText text={description ?? defaults.description} />
          </DialogDescription>
        </DialogHeader>
        {children}
        {requestId ? <RequestId id={requestId} /> : null}
        <DialogFooter>
          {actions ?? (
            <DialogClose asChild>
              <Button variant="secondary">Close</Button>
            </DialogClose>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
