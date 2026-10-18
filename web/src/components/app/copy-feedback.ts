import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

// What a copy button says after a click, and for how long. One state machine for the generic button and the secret one, so
// the two feel the same: a confirmation for a moment, a message that stays long enough to read, and nothing left over when the
// button goes (the timer is cleared on unmount). The text of a message is chosen by the caller; it never holds the copied value.

export const COPIED_MS = 1500;
/** A message with words to read (a failure, a warning about hidden characters) stays longer than "Copied". */
export const MESSAGE_MS = 5000;

export type Feedback = { kind: "idle" } | { kind: "copied" | "problem"; message: string; serial: number };

export const IDLE: Feedback = { kind: "idle" };

export function useCopyFeedback(): { feedback: Feedback; show: (kind: "copied" | "problem", message: string, ms: number) => void; clear: () => void } {
  const [feedback, setFeedback] = useState<Feedback>(IDLE);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const serial = useRef(0);
  // A clipboard that answers after the button has gone must not start a timer nobody will clear.
  const alive = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      clearTimeout(timer.current);
    };
  }, []);

  const clear = useCallback(() => {
    clearTimeout(timer.current);
    setFeedback(IDLE);
  }, []);
  const show = useCallback((kind: "copied" | "problem", message: string, ms: number) => {
    if (!alive.current) return;
    serial.current += 1;
    setFeedback({ kind, message, serial: serial.current });
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      setFeedback(IDLE);
    }, ms);
  }, []);
  return { feedback, show, clear };
}

/** Select the text the person would have copied by hand: an input is focused and selected (Control+C needs the focus); anything else gets a selection range and focus stays where it is. */
export function selectSource(source: RefObject<HTMLElement | null> | undefined): void {
  const element = source?.current;
  if (!element) return;
  if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
    element.focus({ preventScroll: true });
    element.select();
    return;
  }
  const selection = window.getSelection();
  if (!selection) return;
  const range = document.createRange();
  range.selectNodeContents(element);
  selection.removeAllRanges();
  selection.addRange(range);
}
