import { useState, type RefObject } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import { SafeText } from "@/components/app/SafeText";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";
import { REVEAL_VISIBLE_MS } from "./use-reveal";

// "Reveal masked value?" (the File page of the handoff, frame 1e): the setting, a required reason, and the consequence in words,
// that the reveal is recorded in the audit log with the person's name, the setting and the reason. The value itself is not stored in
// the log. The primary button names the action. A refusal stays in the dialog with the reason the person wrote (it is not a secret),
// and nothing is sent again until the button is pressed again.

/** `RevealRequest.reason` is a ShortText. */
export const MAX_REASON_CHARS = 256;

type Props = {
  setting: string;
  working: boolean;
  error: WriteErrorInfo | null;
  /** Where focus goes when the dialog closes with a value shown: the Hide button. Otherwise it goes back to the Reveal button. */
  returnFocusRef: RefObject<HTMLElement | null>;
  onConfirm: (reason: string) => void;
  onCancel: () => void;
};

export function RevealDialog({ setting, working, error, returnFocusRef, onConfirm, onCancel }: Props) {
  const [reason, setReason] = useState("");
  const [invalid, setInvalid] = useState(false);
  const seconds = Math.round(REVEAL_VISIBLE_MS / 1000);

  const submit = (): void => {
    const trimmed = reason.trim();
    if (trimmed === "") {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    onConfirm(trimmed);
  };

  return (
    <ConsequenceDialog
      open
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
      title="Reveal masked value?"
      consequence={`The value is shown on this page for ${seconds} seconds. Revealing it is recorded in the audit log with your name, the setting and your reason.`}
      actionLabel="Reveal value"
      workingLabel="Revealing…"
      initialFocus="first-field"
      working={working}
      error={error}
      returnFocusRef={returnFocusRef}
      onConfirm={submit}
    >
      <div className="flex flex-col gap-3">
        <p className="m-0 font-mono text-xs break-all text-muted-foreground">
          <SafeText text={setting} />
        </p>
        <Field invalid={invalid}>
          <FieldLabel>Reason (required)</FieldLabel>
          <FieldControl>
            <Textarea
              value={reason}
              maxLength={MAX_REASON_CHARS}
              rows={3}
              aria-required="true"
              autoComplete="off"
              onChange={(event) => {
                setReason(event.target.value);
                if (invalid && event.target.value.trim() !== "") setInvalid(false);
              }}
            />
          </FieldControl>
          <FieldError>Enter a reason</FieldError>
          <FieldDescription>Why you need to see it. {MAX_REASON_CHARS} characters at most.</FieldDescription>
        </Field>
        <Alert variant="info">
          <AlertDescription>
            The value is not stored in the log. This page never copies it. If a reveal fails and you try again, that is a second record.
          </AlertDescription>
        </Alert>
      </div>
    </ConsequenceDialog>
  );
}
