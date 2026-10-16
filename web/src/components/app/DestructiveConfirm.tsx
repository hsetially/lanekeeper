import { useEffect, useId, useState } from "react";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import type { dialogSizes } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { assertActionVerb } from "./ConsequenceDialog";
import { CopyButton } from "./CopyButton";
import { SafeText } from "./SafeText";
import { WriteError, type WriteErrorInfo } from "./WriteError";

type DestructiveConfirmProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** What will be lost or changed, said first. */
  consequence: string;
  /** The name that must be typed exactly: a file name, a swimlane id. */
  confirmValue: string;
  /** The destructive verb: "Delete file". */
  actionLabel: string;
  workingLabel?: string;
  cancelLabel?: string;
  onConfirm: () => void;
  working?: boolean;
  error?: WriteErrorInfo | null;
  size?: keyof typeof dialogSizes;
};

/**
 * Delete and similar. The destructive button stays disabled until the exact name is typed; initial focus is on
 * Cancel; Enter never confirms (not in the field, not anywhere): only a click or Space on the button does.
 */
export function DestructiveConfirm({ open, onOpenChange, title, consequence, confirmValue, actionLabel, workingLabel = "Working…", cancelLabel = "Cancel", onConfirm, working = false, error = null, size = "md" }: DestructiveConfirmProps) {
  assertActionVerb(actionLabel);
  const [typed, setTyped] = useState("");
  const fieldId = useId();
  useEffect(() => {
    if (!open) setTyped("");
  }, [open]);
  const matches = typed === confirmValue;
  const enabled = matches && !working;

  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next && working) return;
        onOpenChange(next);
      }}
    >
      <AlertDialogContent size={size}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>
            <SafeText text={consequence} />
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex flex-col gap-1">
          <Label htmlFor={fieldId}>
            Type <SafeText text={confirmValue} className="font-mono" /> to confirm
          </Label>
          <div className="flex items-center gap-1.5">
            <Input
              id={fieldId}
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.preventDefault();
              }}
              disabled={working}
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
            />
            <CopyButton value={confirmValue} what="name" />
          </div>
        </div>
        {error ? <WriteError {...error} /> : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={working}>{cancelLabel}</AlertDialogCancel>
          <Button
            variant="destructive"
            loading={working}
            {...(matches ? {} : { "aria-disabled": true })}
            onClick={() => {
              if (enabled) onConfirm();
            }}
          >
            {working ? workingLabel : actionLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
