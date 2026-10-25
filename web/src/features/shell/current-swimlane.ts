import { useEffect, useSyncExternalStore } from "react";

// The swimlane the person is working in, for the switcher in the header. It lives in memory for the tab and nowhere else
// (plan: "the current swimlane is in memory only"; src/lib/prefs holds layout choices, not what a person is looking at). A
// swimlane screen sets it from its URL, so it follows the page; on a screen that is not a swimlane's, the last one stays.

let current: string | null = null;
const listeners = new Set<() => void>();

function set(next: string | null): void {
  if (current === next) return;
  current = next;
  for (const listener of [...listeners]) listener();
}

const subscribe = (listener: () => void): (() => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};
const snapshot = (): string | null => current;

/** For a test: forget the swimlane. */
export function resetCurrentSwimlane(): void {
  set(null);
}

/** Choose a swimlane (the switcher's list). */
export function chooseSwimlane(id: string): void {
  set(id);
}

/** The current swimlane: the one in the URL when there is one, otherwise the last one chosen or visited. */
export function useCurrentSwimlane(fromRoute: string | null): string | null {
  const remembered = useSyncExternalStore(subscribe, snapshot, snapshot);
  useEffect(() => {
    if (fromRoute !== null) set(fromRoute);
  }, [fromRoute]);
  return fromRoute ?? remembered;
}
