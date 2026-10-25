import { useEffect, useId, useState, type ReactNode, type RefObject } from "react";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import type { dialogSizes } from "@/components/ui/dialog";
import { Check } from "@/icons";
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
  /** What goes between the consequence and the name field: an effect table, a note. */
  children?: ReactNode;
  /** False while the caller is not ready for the write (the effect is not known yet): the button stays off even with the name typed. */
  canConfirm?: boolean;
  /** A warning mark before the title. Decoration: the title says it all. */
  titleIcon?: ReactNode;
  /** A mark in the destructive button. Decoration. */
  actionIcon?: ReactNode;
  /** The design's delete frames: a tick beside the field and a footer line that says what Enter does until the name matches. */
  showMatchHint?: boolean;
  /** Where focus goes on close, when that is not the control that opened the dialog (which is the default, when it is still there). */
  returnFocusRef?: RefObject<HTMLElement | null>;
};

/**
 * Delete and similar. The destructive button stays disabled until the exact name is typed; initial focus is on
 * Cancel; Enter never confirms (not in the field, not on the button, not anywhere): only a click or Space on the button does.
 */
export function DestructiveConfirm({ open, onOpenChange, title, consequence, confirmValue, actionLabel, workingLabel = "Working…", cancelLabel = "Cancel", onConfirm, working = false, error = null, size = "md", children, canConfirm = true, titleIcon, actionIcon, showMatchHint = false, returnFocusRef }: DestructiveConfirmProps) {
  assertActionVerb(actionLabel);
  const [typed, setTyped] = useState("");
  const fieldId = useId();
  useEffect(() => {
    if (!open) setTyped("");
  }, [open]);
  const matches = typed === confirmValue;
  const enabled = matches && canConfirm && !working;

  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next && working) return;
        onOpenChange(next);
      }}
    >
      <AlertDialogContent size={size} {...(returnFocusRef ? { returnFocusRef } : {})}>
        <AlertDialogHeader>
          <AlertDialogTitle className={titleIcon ? "flex items-center gap-2" : undefined}>
            {titleIcon ? (
              <span aria-hidden="true" className="inline-flex shrink-0 text-danger-fg">
                {titleIcon}
              </span>
            ) : null}
            <SafeText text={title} />
          </AlertDialogTitle>
          <AlertDialogDescription>
            <SafeText text={consequence} />
          </AlertDialogDescription>
        </AlertDialogHeader>
        {children}
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
            {showMatchHint && matches ? <Check aria-hidden="true" className="size-3.5 shrink-0 text-success-fg" /> : null}
            <CopyButton value={confirmValue} what="name" />
          </div>
        </div>
        {error ? <WriteError {...error} /> : null}
        <AlertDialogFooter>
          {showMatchHint ? <span className="mr-auto text-xs text-muted-foreground">{matches ? "Name matches" : "Enter does nothing until the name matches"}</span> : null}
          <AlertDialogCancel disabled={working}>{cancelLabel}</AlertDialogCancel>
          <Button
            variant="destructive"
            loading={working}
            {...(matches && canConfirm ? {} : { "aria-disabled": true })}
            // A button's own Enter is a click; here it is not (the keymap's row for dialogs: never in DestructiveConfirm). A click
            // and Space still confirm.
            onKeyDown={(event) => {
              if (event.key === "Enter") event.preventDefault();
            }}
            onClick={() => {
              if (enabled) onConfirm();
            }}
          >
            {actionIcon ? (
              <span aria-hidden="true" className="inline-flex shrink-0">
                {actionIcon}
              </span>
            ) : null}
            {working ? workingLabel : actionLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
