import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { ShortcutRegistry, type Allow, type ShortcutHandler } from "./registry";
import type { ShortcutId } from "./rows";

// The registry, mounted once at the root of the app shell, and the one hook a screen uses to bind a handler to a row of the
// keymap. Screens never read the registry or add a listener of their own: `useShortcut` is the whole interface, and
// src/keyboard/** is not theirs to edit.
//
// The listener is on `window`, in the CAPTURE phase, for the reason in registry.ts (Monaco stops the keys it binds).

const KeyboardContext = createContext<ShortcutRegistry | null>(null);

export function KeyboardProvider({ children, registry }: { children: ReactNode; registry?: ShortcutRegistry }) {
  // One registry for the life of the provider. A test passes its own, with a clock it controls.
  const [own] = useState(() => registry ?? new ShortcutRegistry());
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      own.handle(event);
    };
    // A half-typed sequence does not survive the page losing focus.
    const onBlur = (): void => {
      own.cancel();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", onBlur);
    };
  }, [own]);
  return <KeyboardContext.Provider value={own}>{children}</KeyboardContext.Provider>;
}

/** The registry, for the shell's own pieces (the sheet lists what is bound). Screens use `useShortcut`. */
export function useShortcutRegistry(): ShortcutRegistry | null {
  return useContext(KeyboardContext);
}

export type UseShortcutOptions = {
  /** False: the row is not bound (the screen is not in the state where the key means anything). Default true. */
  enabled?: boolean;
  /** Higher wins over another handler of the same key. Default: the shell's rows 0, a screen's 1. */
  layer?: number;
  /** Where the key fires, beyond the defaults (registry.ts). */
  allow?: Allow;
};

/**
 * Bind `handler` to a row of the keymap while the component is mounted. The handler gets which binding fired (a row such as
 * "Go to" has five) and the event; return `false` to decline the key. Without a KeyboardProvider (a test of a single
 * component) it binds nothing.
 */
export function useShortcut(id: ShortcutId, handler: ShortcutHandler, options: UseShortcutOptions = {}): void {
  const registry = useContext(KeyboardContext);
  const latest = useRef(handler);
  useEffect(() => {
    latest.current = handler;
  });
  const { enabled = true, layer, allow } = options;
  const inInput = allow?.input;
  const inDialog = allow?.dialog;
  const inEditor = allow?.editor;
  useEffect(() => {
    if (!registry || !enabled) return undefined;
    return registry.register(id, (hit) => latest.current(hit), {
      ...(layer === undefined ? {} : { layer }),
      allow: { ...(inInput === undefined ? {} : { input: inInput }), ...(inDialog === undefined ? {} : { dialog: inDialog }), ...(inEditor === undefined ? {} : { editor: inEditor }) },
    });
  }, [registry, id, enabled, layer, inInput, inDialog, inEditor]);
}
