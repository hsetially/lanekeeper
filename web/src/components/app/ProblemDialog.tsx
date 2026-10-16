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
};

export function ProblemDialog({ open, onOpenChange, variant, title, description, requestId, children, actions }: ProblemDialogProps) {
  const defaults = DEFAULTS[variant];
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="lg">
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
