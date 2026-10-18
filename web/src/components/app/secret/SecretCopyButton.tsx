import type { RefObject } from "react";
import { Button } from "@/components/ui/button";
import { Check, Copy } from "@/icons";
import { writeClipboard } from "../clipboard";
import { COPIED_MS, MESSAGE_MS, selectSource, useCopyFeedback } from "../copy-feedback";
import { Announce, StackedWord } from "../copy-parts";
import { COULD_NOT_COPY, NOTHING_TO_COPY, checkCopyable } from "../copy-text";

// The copy button of a ONE-TIME SECRET: the admin join token and a personal MCP token, in the two displays that show them once
// (features/admin/swimlanes/JoinTokenCard.tsx, components/app/PersonalTokens.tsx). Nothing else imports this file: a lint rule
// (lint/restricted-imports.mjs) and secret-imports.test.ts say so. A GitHub token the person TYPES (My token) is not output
// and has no copy button; a revealed setting value has none either (a revealed value is never copied: plan, "Masking and
// secrets in the browser").
//
// Stricter than CopyButton, on purpose:
//  - A click is the only thing that writes. Nothing runs on mount or on a timer, and with no user activation (the browser would
//    refuse the write; a script calling click() has none) it does not try.
//  - The value is not a prop. `getValue` is called once, inside the click, and its answer lives in that function and in the
//    clipboard call, nowhere else: not in state, not in a key, a data attribute, a title or the accessible name. What the
//    button says ("Copied. It will not be shown again.") never includes it.
//  - It never logs, and it is not a toast.
//  - It is a child of the secret display, so it goes when the secret is dismissed or the display unmounts, and its timer with it.
//  - A masked value has no clipboard text: an empty value or a redaction marker is refused ("Nothing to copy"). The one-time
//    displays never hold a masked value; this is the guard if one ever did.
//
// What it cannot do, and the threat model says so (docs/threat-model.md, "Copying a one-time secret"): another application, or a
// clipboard history, can read the clipboard after the write, and the page cannot take the secret back out (writing an empty
// string later could overwrite something else the person copied since). The token is shown once and revocable.

export const SECRET_COPIED = "Copied. It will not be shown again.";

/** The marker the hub writes where a value is masked (M1). Its text is not a value. */
const REDACTION_MARKER = "[REDACTED:";

type SecretCopyButtonProps = {
  /** Called once, inside the click, for the text to copy. Never called on mount or render. */
  getValue: () => string;
  /** What is copied, for the name: "join token" gives "Copy join token". */
  what: string;
  /** The field that shows the secret: it is focused and selected when the copy fails, so Control+C does what the button could not. */
  source?: RefObject<HTMLInputElement | HTMLTextAreaElement | null>;
  className?: string;
};

type UserActivation = { readonly isActive: boolean };

export function SecretCopyButton({ getValue, what, source, className }: SecretCopyButtonProps) {
  const { feedback, show } = useCopyFeedback();

  const fail = (message: string): void => {
    show("problem", message, MESSAGE_MS);
    selectSource(source);
  };

  const copy = (): void => {
    const activation = (navigator as { userActivation?: UserActivation }).userActivation;
    if (activation !== undefined && !activation.isActive) {
      fail(COULD_NOT_COPY);
      return;
    }
    const value = getValue();
    const check = checkCopyable(value);
    if (!check.ok) {
      show("problem", check.message, MESSAGE_MS);
      return;
    }
    if (value.includes(REDACTION_MARKER)) {
      show("problem", NOTHING_TO_COPY, MESSAGE_MS);
      return;
    }
    void writeClipboard(value).then((result) => {
      if (result === "ok") show("copied", SECRET_COPIED, COPIED_MS);
      else fail(COULD_NOT_COPY);
    });
  };

  const copied = feedback.kind === "copied";
  const name = `Copy ${what}`;
  return (
    <>
      <Button type="button" variant="secondary" data-slot="secret-copy-button" data-copy={feedback.kind} aria-label={name} onClick={copy} className={className}>
        <span className="grid">
          <StackedWord active={!copied} icon={<Copy aria-hidden="true" />} text={name} />
          <StackedWord active={copied} icon={<Check data-slot="copy-check" aria-hidden="true" className="animate-lk-pop-in text-success-fg" />} text="Copied" />
        </span>
      </Button>
      {feedback.kind === "problem" ? (
        <span aria-hidden="true" className="text-xs text-danger-fg">
          {feedback.message}
        </span>
      ) : null}
      <Announce feedback={feedback} />
    </>
  );
}
