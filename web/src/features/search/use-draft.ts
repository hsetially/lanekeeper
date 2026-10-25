import { useCallback, useEffect, useRef, useState } from "react";

// A field whose text goes into the address (and so into a request) a moment after the person stops typing. The address is the
// single source of truth: when it changes by itself (Back, a suggestion, a link) the field follows it; when it changes because
// of what was typed, the field is left alone. Enter does not wait.

export function useDraft(urlValue: string, commit: (value: string) => void, delayMs = 250): { value: string; set: (value: string) => void; flush: () => void } {
  const [draft, setDraft] = useState(urlValue);
  // What the address holds because of this field.
  const pushed = useRef(urlValue);
  const latest = useRef(commit);
  useEffect(() => {
    latest.current = commit;
  });

  useEffect(() => {
    if (urlValue === pushed.current) return;
    pushed.current = urlValue;
    setDraft(urlValue);
  }, [urlValue]);

  useEffect(() => {
    if (draft === pushed.current) return undefined;
    const timer = setTimeout(() => {
      pushed.current = draft;
      latest.current(draft);
    }, delayMs);
    return () => {
      clearTimeout(timer);
    };
  }, [draft, delayMs]);

  const flush = useCallback(() => {
    if (draft === pushed.current) return;
    pushed.current = draft;
    latest.current(draft);
  }, [draft]);

  return { value: draft, set: setDraft, flush };
}
