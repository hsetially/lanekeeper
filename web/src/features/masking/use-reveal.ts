import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import type { PlannedMark } from "@/components/masking/plan";
import { useSecretMutation } from "@/lib/query/secret-mutation";
import { revealError } from "./reveal-error";
import { revealValueRequest } from "./reveal-request";

// The state of a Reveal (S14b, D79; plan "Masking and secrets in the browser"). `revealValue` is audited server-side, so every rule
// here is about the value, the one secret this page can ever hold of a file:
//
//  - the request goes through useSecretMutation only: gcTime 0, removed and reset in a `finally`, retry false (a retry is a second
//    audited reveal), and the answer reaches this hook through the promise and nowhere else;
//  - the value lives in this component's state and nowhere else: not the query cache, not the mutation cache, not storage, not a URL,
//    not a log, not a toast, not an attribute or a name, and never the clipboard (this page has no copy for it);
//  - it is shown for REVEAL_VISIBLE_MS at most, and it is cleared on every outcome that is not a success, when the person hides it,
//    when the window loses focus or the tab is hidden, when the text or the file it was revealed in changes, and on unmount (an answer
//    that arrives after that is dropped, and the request is aborted);
//  - only one value is shown at a time;
//  - nothing sends a second request by itself: a failure leaves the dialog open and the person presses the button again.

/** How long a revealed value stays on the page (the plan: about 30 seconds). */
export const REVEAL_VISIBLE_MS = 30_000;
/** A value is drawn in a row of one line; past this it is clipped and the row says so. */
export const MAX_SHOWN_CHARS = 2_000;

export type Revealed = { readonly key: string; readonly value: string; readonly clipped: boolean };

type Where = { swimlane: string; path: string; text: string };

export type RevealState = {
  /** The mark whose dialog is open, or null. */
  readonly target: PlannedMark | null;
  readonly revealed: Revealed | null;
  readonly working: boolean;
  readonly error: WriteErrorInfo | null;
  /** What a screen reader is told, politely, when the state changes. Never the value. */
  readonly announcement: string;
  /** Open the dialog for a mark that names a setting. */
  readonly request: (mark: PlannedMark) => void;
  readonly cancel: () => void;
  /** Send the reveal. Does nothing while one is in flight. */
  readonly confirm: (reason: string) => void;
  readonly hide: () => void;
  /** The element that has focus when the value is hidden gets focus back through this key (see RevealableMaskedText). */
  readonly consumeRefocus: (key: string) => boolean;
  readonly hideRef: RefObject<HTMLButtonElement | null>;
};

export function useReveal(where: Where): RevealState {
  const { swimlane, path, text } = where;
  const { run } = useSecretMutation(revealValueRequest);
  const [target, setTarget] = useState<PlannedMark | null>(null);
  const [revealed, setRevealed] = useState<Revealed | null>(null);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<WriteErrorInfo | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const [seen, setSeen] = useState<Where>(where);

  const live = useRef(false);
  const busy = useRef(false);
  const abort = useRef<AbortController | null>(null);
  const latest = useRef<Where>(where);
  const refocus = useRef<string | null>(null);
  const hideRef = useRef<HTMLButtonElement | null>(null);

  // Another file, or another text of it: whatever was revealed belongs to the old one, and a dialog for a mark of it is for the old
  // one too. Reset while rendering, so that the new text is never drawn next to the old value.
  if (seen.swimlane !== swimlane || seen.path !== path || seen.text !== text) {
    setSeen(where);
    setRevealed(null);
    setTarget(null);
    setError(null);
    setAnnouncement("");
  }

  useEffect(() => {
    latest.current = where;
  });

  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
      abort.current?.abort();
    };
  }, []);

  /** Take the value off the page. Focus on the Hide button goes to the Reveal button that takes its place. */
  const forget = useCallback((message: string) => {
    const button = hideRef.current;
    if (button !== null && document.activeElement === button) refocus.current = button.dataset["maskKey"] ?? null;
    setRevealed(null);
    setAnnouncement(message);
  }, []);

  // The value goes after a while, and when the person is no longer looking at the page.
  const shown = revealed !== null;
  useEffect(() => {
    if (!shown) return undefined;
    const timer = setTimeout(() => forget("Value hidden."), REVEAL_VISIBLE_MS);
    const away = (): void => {
      if (document.visibilityState === "hidden") forget("Value hidden.");
    };
    const blurred = (): void => forget("Value hidden.");
    document.addEventListener("visibilitychange", away);
    window.addEventListener("blur", blurred);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", away);
      window.removeEventListener("blur", blurred);
    };
  }, [shown, forget]);

  const request = useCallback((mark: PlannedMark) => {
    if (mark.setting === null) return;
    setError(null);
    setTarget(mark);
  }, []);

  const cancel = useCallback(() => {
    if (busy.current) return;
    setTarget(null);
    setError(null);
  }, []);

  const confirm = useCallback(
    (reason: string) => {
      const mark = target;
      if (mark === null || mark.setting === null || busy.current) return;
      busy.current = true;
      setWorking(true);
      setError(null);
      const controller = new AbortController();
      abort.current = controller;
      const asked = { swimlane, path, text };
      run({ swimlane, body: { path, setting: mark.setting, reason }, signal: controller.signal })
        .then((value) => {
          const now = latest.current;
          // The component went, or the file moved under the request: the answer is for something that is no longer here.
          if (!live.current || now.swimlane !== asked.swimlane || now.path !== asked.path || now.text !== asked.text) return;
          const clipped = value.length > MAX_SHOWN_CHARS;
          setRevealed({ key: mark.key, value: clipped ? value.slice(0, MAX_SHOWN_CHARS) : value, clipped });
          setAnnouncement(`Value revealed. It is hidden again after ${Math.round(REVEAL_VISIBLE_MS / 1000)} seconds, when you hide it, or when you leave this window.`);
          setTarget(null);
        })
        .catch((failure: unknown) => {
          if (!live.current || controller.signal.aborted) return;
          setError(revealError(failure));
        })
        .finally(() => {
          busy.current = false;
          abort.current = null;
          if (live.current) setWorking(false);
        });
    },
    [target, swimlane, path, text, run],
  );

  const hide = useCallback(() => forget("Value hidden."), [forget]);

  const consumeRefocus = useCallback((key: string): boolean => {
    if (refocus.current !== key) return false;
    refocus.current = null;
    return true;
  }, []);

  return { target, revealed, working, error, announcement, request, cancel, confirm, hide, consumeRefocus, hideRef };
}
