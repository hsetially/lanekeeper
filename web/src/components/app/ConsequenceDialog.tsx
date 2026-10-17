import type { ReactNode, SyntheticEvent } from "react";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import type { dialogSizes } from "@/components/ui/dialog";
import { SafeText } from "./SafeText";
import { WriteError, type WriteErrorInfo } from "./WriteError";

/** "OK" and friends say nothing about what will happen. The primary button names the action. */
const NOT_A_VERB = /^(ok|okay|yes|no|done|continue)$/i;

export function assertActionVerb(label: string): void {
  if (import.meta.env.DEV && NOT_A_VERB.test(label.trim())) {
    throw new Error(`The primary action label must be a verb that names the action ("Raise PR", "Restart 2 services"), not "${label}".`);
  }
}

type ConsequenceDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** What will happen, said first: "2 services need restart". Shown as the description. */
  consequence: string;
  /** The details and form fields. */
  children?: ReactNode;
  /** The action verb: "Raise PR". Never "OK". */
  actionLabel: string;
  /** Shown on the primary button while the write runs: "Raising…". */
  workingLabel?: string;
  cancelLabel?: string;
  /** Called on a click or on Enter in a field, only when the primary action is enabled. */
  onConfirm: () => void;
  /** The write is running: buttons stop, Escape does not close. Nothing in the page changes until the server answers. */
  working?: boolean;
  /** False while the form is invalid. The button stays focusable and says nothing is sent. */
  canConfirm?: boolean;
  /** A failed write stays in the dialog with the user's input intact. */
  error?: WriteErrorInfo | null;
  size?: keyof typeof dialogSizes;
};

/**
 * The shell of every write that has a consequence: Raise PR, Restart services, Adopt swimlane, Save with impact.
 * The consequence comes first, the primary button names the action, initial focus is on Cancel, a write in
 * progress locks the dialog, and an error is shown inline with its request ID (never as a toast).
 * The state (working, error) is the caller's, from the mutation: writes are not optimistic.
 */
export function ConsequenceDialog({ open, onOpenChange, title, consequence, children, actionLabel, workingLabel = "Working…", cancelLabel = "Cancel", onConfirm, working = false, canConfirm = true, error = null, size = "md" }: ConsequenceDialogProps) {
  assertActionVerb(actionLabel);
  const handleOpenChange = (next: boolean) => {
    if (!next && working) return;
    onOpenChange(next);
  };
  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    if (working || !canConfirm) return;
    onConfirm();
  };
  return (
    <AlertDialog open={open} onOpenChange={handleOpenChange}>
      <AlertDialogContent size={size}>
        <form onSubmit={submit} className="flex flex-col gap-3">
          <AlertDialogHeader>
            <AlertDialogTitle>{title}</AlertDialogTitle>
            <AlertDialogDescription>
              <SafeText text={consequence} />
            </AlertDialogDescription>
          </AlertDialogHeader>
          {children}
          {error ? <WriteError {...error} /> : null}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={working}>{cancelLabel}</AlertDialogCancel>
            <Button type="submit" loading={working} {...(canConfirm ? {} : { "aria-disabled": true })}>
              {working ? workingLabel : actionLabel}
            </Button>
          </AlertDialogFooter>
        </form>
      </AlertDialogContent>
    </AlertDialog>
  );
}
