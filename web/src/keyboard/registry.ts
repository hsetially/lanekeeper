// The shortcut registry: one place that decides what a key press means, so that two screens can never bind the same key
// behind each other's back and every key follows the same rules about typing, dialogs and the editor.
//
//  - A handler is bound to a ROW of the keymap (rows.ts), never to a key. The row says which keys (and sequences) fire it.
//  - The registry listens once, on `window`, in the CAPTURE phase (src/keyboard/KeyboardProvider.tsx). Monaco stops
//    propagation of the keys it binds, so a bubble-phase listener would never see ⌘K there; capture sees it first.
//  - A single key or a sequence (`g` then `s`) does not fire while a text field or the editor has focus, nor while a dialog or
//    a menu is open. A chord (Control or Command plus a key) fires in a field and in the editor, not in a dialog. Escape is
//    delivered everywhere except the editor and a dialog (the dialog's own Esc closes it). A registration can widen this
//    with `allow`.
//  - Several handlers for one key: the highest LAYER wins (a screen's row over the shell's), then the latest registered. A
//    handler that returns `false` declines, and the next one is asked, then the page.
//  - A sequence waits SEQUENCE_MS for its second key. The clock is injected, so a test never waits.
//  - Everything is bounded (rule 5): at most MAX_REGISTRATIONS handlers at once.

import { matchesStep, type KeyLike, type KeyStep } from "./keys";
import { bindingsOf, rowById, type ParsedBinding, type ShortcutId } from "./rows";

export const SEQUENCE_MS = 1200;
export const MAX_REGISTRATIONS = 64;

export type ShortcutHit = { readonly id: ShortcutId; /** Index of the binding that fired, in the row's `bindings`. */ readonly binding: number; readonly action: string; readonly event: KeyboardEvent };
/** Return `false` (and only `false`) to decline: the key then goes on to the next handler and to the page. */
export type ShortcutHandler = (hit: ShortcutHit) => unknown;

export type Allow = { input?: boolean; dialog?: boolean; editor?: boolean };
export type RegisterOptions = {
  /** Higher wins. Default: 0 for the shell's rows (Global, Go to), 1 for every screen's. */
  layer?: number;
  /** Where the key fires. The defaults are in the file header. */
  allow?: Allow;
};

export type Environment = {
  now: () => number;
  /** The element that has focus. */
  active: () => Element | null;
  /** Whether a dialog, alert dialog or menu is open. */
  overlayOpen: () => boolean;
};

const INPUT_TYPES_WITHOUT_TEXT = new Set(["button", "checkbox", "radio", "range", "color", "file", "image", "reset", "submit"]);
const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "AltGraph", "CapsLock", "NumLock", "ScrollLock", "Fn", "FnLock", "Hyper", "Super", "Symbol", "SymbolLock", "OS"]);
const SHELL_SCOPES = new Set(["Global", "Go to"]);

/** True when typing here means characters: an input, a textarea, a select, or anything editable. */
export function isTypingTarget(element: Element | null): boolean {
  if (!(element instanceof HTMLElement)) return false;
  if (element.isContentEditable || element.getAttribute("contenteditable") === "" || element.getAttribute("contenteditable") === "true") return true;
  const tag = element.tagName;
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag === "INPUT") return !INPUT_TYPES_WITHOUT_TEXT.has((element as HTMLInputElement).type);
  const role = element.getAttribute("role");
  return role === "textbox" || role === "searchbox" || role === "combobox" || role === "spinbutton";
}

export function isInEditor(element: Element | null): boolean {
  return element !== null && element.closest(".monaco-editor") !== null;
}

const OVERLAY = '[role="dialog"][data-state="open"], [role="alertdialog"][data-state="open"], [role="menu"][data-state="open"]';

function browserEnvironment(): Environment {
  return {
    now: () => Date.now(),
    active: () => document.activeElement,
    overlayOpen: () => document.querySelector(OVERLAY) !== null,
  };
}

type Registration = { readonly id: ShortcutId; readonly handler: ShortcutHandler; readonly layer: number; readonly allow: Allow; readonly order: number };
type Candidate = { readonly registration: Registration; readonly binding: ParsedBinding };

function defaultLayer(id: string): number {
  const scope = rowById(id)?.scope ?? "";
  return SHELL_SCOPES.has(scope) ? 0 : 1;
}

/** Where a binding fires, from its first step, unless the registration said otherwise. */
function allowed(first: KeyStep, allow: Allow, where: { input: boolean; dialog: boolean; editor: boolean }): boolean {
  const escape = first.key === "Escape";
  const chord = first.ctrlOrMeta;
  const input = allow.input ?? (chord || escape);
  const editor = allow.editor ?? chord;
  const dialog = allow.dialog ?? false;
  if (where.dialog && !dialog) return false;
  if (where.editor && !editor) return false;
  if (where.input && !input) return false;
  return true;
}

export class ShortcutRegistry {
  private readonly registrations = new Map<ShortcutId, Registration[]>();
  private readonly env: Environment;
  private readonly sequenceMs: number;
  private count = 0;
  private serial = 0;
  private armed: { candidates: readonly Candidate[]; index: number; at: number } | null = null;

  constructor(env: Partial<Environment> = {}, options: { sequenceMs?: number } = {}) {
    this.env = { ...browserEnvironment(), ...env };
    this.sequenceMs = options.sequenceMs ?? SEQUENCE_MS;
  }

  /** Bind a handler to a row. Returns the function that removes it. */
  register(id: ShortcutId, handler: ShortcutHandler, options: RegisterOptions = {}): () => void {
    const row = rowById(id);
    if (!row) throw new Error(`no shortcut row ${id}`);
    if (bindingsOf(id).length === 0) throw new Error(`shortcut row ${id} has no key to bind (type-ahead and unassigned rows are handled by their component)`);
    if (this.count >= MAX_REGISTRATIONS) throw new Error("too many shortcut handlers");
    const registration: Registration = { id, handler, layer: options.layer ?? defaultLayer(id), allow: options.allow ?? {}, order: this.serial++ };
    const list = this.registrations.get(id) ?? [];
    list.push(registration);
    this.registrations.set(id, list);
    this.count += 1;
    let removed = false;
    return () => {
      if (removed) return;
      removed = true;
      const current = this.registrations.get(id);
      if (!current) return;
      const at = current.indexOf(registration);
      if (at >= 0) current.splice(at, 1);
      if (current.length === 0) this.registrations.delete(id);
      this.count -= 1;
      // A sequence that was waiting for a handler that is gone must not fire anything.
      if (this.armed?.candidates.some((c) => c.registration === registration)) this.armed = null;
    };
  }

  /** The rows that have a handler now (the shortcuts sheet and the tests). */
  registered(): ShortcutId[] {
    return [...this.registrations.keys()];
  }

  /** Forget a half-typed sequence (the page lost focus, or the route changed). */
  cancel(): void {
    this.armed = null;
  }

  /** Whether a sequence is waiting for its second key. */
  get pending(): boolean {
    return this.armed !== null;
  }

  /** Handle a key press. True when a handler took it (it is then prevented and stopped). */
  handle(event: KeyboardEvent): boolean {
    if (event.isComposing || event.key === "Process" || event.key === "Dead" || event.key === "Unidentified") return false;
    // A modifier on its own is never a shortcut and never breaks a half-typed sequence.
    if (MODIFIER_KEYS.has(event.key)) return false;
    // Holding a key down must not run a command again and again.
    if (event.repeat) return false;
    const now = this.env.now();
    const active = this.env.active();
    const where = { input: isTypingTarget(active), editor: isInEditor(active), dialog: this.env.overlayOpen() };

    if (this.armed) {
      const { candidates, index, at } = this.armed;
      this.armed = null;
      if (now - at <= this.sequenceMs && !where.dialog) {
        const next = candidates.filter((c) => c.binding.steps[index] !== undefined && matchesStep(c.binding.steps[index], event as KeyLike));
        if (next.length > 0) {
          const done = next.filter((c) => c.binding.steps.length === index + 1);
          if (done.length > 0) return this.fire(done, event);
          this.armed = { candidates: next, index: index + 1, at: now };
          event.preventDefault();
          event.stopPropagation();
          return true;
        }
      }
      // A wrong second key, or a late one, cancels the sequence and is then an ordinary key.
    }

    const fresh: Candidate[] = [];
    for (const list of this.registrations.values()) {
      for (const registration of list) {
        for (const binding of bindingsOf(registration.id)) {
          const first = binding.steps[0];
          if (first === undefined || !matchesStep(first, event)) continue;
          if (!allowed(first, registration.allow, where)) continue;
          fresh.push({ registration, binding });
        }
      }
    }
    if (fresh.length === 0) return false;
    const single = fresh.filter((c) => c.binding.steps.length === 1);
    if (single.length > 0) return this.fire(single, event);
    this.armed = { candidates: fresh, index: 1, at: now };
    event.preventDefault();
    event.stopPropagation();
    return true;
  }

  private fire(candidates: readonly Candidate[], event: KeyboardEvent): boolean {
    const ordered = [...candidates].sort((a, b) => b.registration.layer - a.registration.layer || b.registration.order - a.registration.order);
    for (const { registration, binding } of ordered) {
      const result = registration.handler({ id: registration.id, binding: binding.index, action: binding.action, event });
      if (result === false) continue;
      event.preventDefault();
      event.stopPropagation();
      return true;
    }
    return false;
  }
}
