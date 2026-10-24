import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import type { Feedback } from "./copy-feedback";

// The two pieces both copy buttons (CopyButton, secret/SecretCopyButton) are built from, so that they behave the same way.

/**
 * One of the two words of a labelled button ("Copy token", "Copied"). Both are laid out in one grid cell, so the button is as
 * wide in either state and nothing moves; the one that is not showing is hidden from view and from assistive technology.
 */
export function StackedWord({ active, icon, text }: { active: boolean; icon: ReactNode; text: string }) {
  return (
    <span aria-hidden={active ? undefined : true} className={cn("col-start-1 row-start-1 inline-flex items-center justify-center gap-1.5", !active && "invisible")}>
      {icon}
      {text}
    </span>
  );
}

/**
 * The live region. It is always in the page, so a screen reader is already listening when the words arrive; each message is a new
 * node, so a second one is said again. Visually hidden: the sighted person has the bubble or the label.
 */
export function Announce({ feedback }: { feedback: Feedback }) {
  return (
    <span role="status" className="sr-only">
      {feedback.kind === "idle" ? null : <span key={feedback.serial}>{feedback.message}</span>}
    </span>
  );
}
