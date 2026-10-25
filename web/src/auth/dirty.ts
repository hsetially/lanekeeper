import { useEffect } from "react";

// The dirty-edit registry. A screen with unsaved work (an editor with changes, a dialog with typed text) registers here.
// Three things read it: the router blocker and `beforeunload` (leaving the page would lose the work), and the session
// handler (a 401 while work is open must not navigate away: it shows "session ended" and keeps the page, so the person can
// copy their draft first). Bounded to 16 entries (rule 5).

export type DirtyEntry = {
  /** The text a person would want to keep, for "Copy draft". Called only when a session ends. */
  getDraft?: () => string;
};

const MAX_ENTRIES = 16;
const entries = new Map<string, DirtyEntry>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of [...listeners]) listener();
}

/** Register unsaved work under `id`. Returns the function that clears it. */
export function markDirty(id: string, entry: DirtyEntry = {}): () => void {
  if (!entries.has(id) && entries.size >= MAX_ENTRIES) return () => undefined;
  entries.set(id, entry);
  notify();
  return () => {
    entries.delete(id);
    notify();
  };
}

export function hasDirty(): boolean {
  return entries.size > 0;
}

/** The drafts of everything that is dirty, joined for one copy. Empty when nothing offers one. */
export function collectDrafts(): string {
  return [...entries.values()]
    .map((entry) => entry.getDraft?.() ?? "")
    .filter((text) => text !== "")
    .join("\n\n");
}

export function subscribeDirty(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** For a component: registered while `dirty` is true and the component is mounted. */
export function useDirty(id: string, dirty: boolean, getDraft?: () => string): void {
  useEffect(() => {
    if (!dirty) return undefined;
    return markDirty(id, getDraft ? { getDraft } : {});
  }, [id, dirty, getDraft]);
}

/** For tests. */
export function clearDirtyForTests(): void {
  entries.clear();
  notify();
}
