import { useCallback, useRef, useState } from "react";
import { useDirty } from "@/auth/dirty";

// What editing a file remembers. `base` is the version the person was looking at when they pressed Edit (its hash and text); a
// refresh of the file behind it never moves it, because a save must say which version it is based on and the person must
// have seen that version. `rebase` is the one way it moves, and only when the person chose "Reload and re-apply" after a 409:
// the text they typed stays, and the next save names the version that is on NFS now.
//
// While there are unsaved changes the work is registered with the dirty registry (src/auth/dirty.ts), so that leaving the
// page asks first and a session that ends keeps the page and offers the draft to copy.

export type EditBase = { hash: string | null; text: string };
export type EditSession = { base: EditBase; text: string; rebasedFrom: string | null };

export function useEditSession() {
  const [session, setSession] = useState<EditSession | null>(null);
  // What was set most recently, kept in step with the state at the moment it is set. The state is drawn on React's turn, and a
  // key that arrives before that turn (Escape right after a letter) is answered by a handler made in an earlier render: it asks
  // here (`isDirty`, `draft`) and not at its own render's `dirty`.
  const latest = useRef<EditSession | null>(null);
  const isDirty = useCallback(() => latest.current !== null && latest.current.text !== latest.current.base.text, []);
  const dirty = session !== null && session.text !== session.base.text;
  const draft = useCallback(() => latest.current?.text ?? "", []);
  useDirty("file-edit", dirty, draft);

  const set = useCallback((next: EditSession | null) => {
    latest.current = next;
    setSession(next);
  }, []);
  const start = useCallback((base: EditBase) => set({ base, text: base.text, rebasedFrom: null }), [set]);
  const setText = useCallback(
    (next: string) => {
      const s = latest.current;
      if (s !== null && s.text !== next) set({ ...s, text: next });
    },
    [set],
  );
  const rebase = useCallback(
    (hash: string | null) => {
      const s = latest.current;
      if (s !== null) set({ ...s, base: { ...s.base, hash }, rebasedFrom: s.rebasedFrom ?? s.base.hash });
    },
    [set],
  );
  const end = useCallback(() => set(null), [set]);

  return { session, dirty, isDirty, start, setText, rebase, end };
}
