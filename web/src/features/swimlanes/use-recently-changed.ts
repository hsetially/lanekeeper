import { useEffect, useRef, useState } from "react";

// Which rows of a live list changed in place since the last time the list was drawn, for a few seconds: the design highlights the
// swimlane that "just changed". The first list a screen gets is not a change, and a row that is new to the list is the stable
// list's business (the "n updates" chip), not a highlight. Bounded: a row is remembered once, for `ms`, and the timers go with
// the component.

export const HIGHLIGHT_MS = 8000;

export function useRecentlyChanged<T>(rows: readonly T[], keyOf: (row: T) => string, signatureOf: (row: T) => string, ms = HIGHLIGHT_MS): ReadonlySet<string> {
  const previous = useRef<Map<string, string> | null>(null);
  const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const [changed, setChanged] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    const next = new Map<string, string>();
    const moved: string[] = [];
    for (const row of rows) {
      const key = keyOf(row);
      const signature = signatureOf(row);
      next.set(key, signature);
      const before = previous.current?.get(key);
      if (before !== undefined && before !== signature) moved.push(key);
    }
    previous.current = next;
    if (moved.length === 0) return;
    setChanged((current) => new Set([...current, ...moved]));
    for (const key of moved) {
      const running = timers.current.get(key);
      if (running !== undefined) clearTimeout(running);
      timers.current.set(
        key,
        setTimeout(() => {
          timers.current.delete(key);
          setChanged((current) => {
            const rest = new Set(current);
            rest.delete(key);
            return rest;
          });
        }, ms),
      );
    }
  }, [rows, keyOf, signatureOf, ms]);

  useEffect(() => {
    const running = timers.current;
    return () => {
      for (const timer of running.values()) clearTimeout(timer);
      running.clear();
    };
  }, []);

  return changed;
}
