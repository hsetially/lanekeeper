// One key press, as the handoff's keymap writes it, and the comparison with a browser event.
//
// The steps come from src/handoff/keymap.generated.ts and use Playwright's spelling: `ControlOrMeta+k`, `Shift+Tab`, `Escape`,
// `Space`, `/`, `?`, `ArrowUp`. `ControlOrMeta` is Control or Command, whichever the person's keyboard has: the handoff draws
// the Command glyph, and a Windows or Linux user presses Control for the same thing.

/** What the comparison needs of a KeyboardEvent, so a test can pass a plain object. */
export type KeyLike = { key: string; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean };

export type KeyStep = { key: string; ctrlOrMeta: boolean; shift: boolean; alt: boolean };

const MODIFIERS = new Set(["ControlOrMeta", "Shift", "Alt"]);

/** `Space` is the one named key whose `KeyboardEvent.key` is not its name. */
const KEY_VALUE: Record<string, string> = { Space: " " };

export function parseStep(step: string): KeyStep {
  // A step is modifiers joined to a key by `+`; the key itself can be `+` only as the last part, so split from the left.
  const parts: string[] = [];
  let rest = step;
  for (;;) {
    const at = rest.indexOf("+");
    if (at <= 0 || at === rest.length - 1) break;
    parts.push(rest.slice(0, at));
    rest = rest.slice(at + 1);
  }
  const key = rest;
  // `Ctrl+` has no key; a lone `+` or `Ctrl++` is the plus key.
  if (key === "" || MODIFIERS.has(key) || (key.length > 1 && key.endsWith("+"))) throw new Error(`unreadable key step: "${step}"`);
  const out: KeyStep = { key: KEY_VALUE[key] ?? key, ctrlOrMeta: false, shift: false, alt: false };
  for (const part of parts) {
    if (!MODIFIERS.has(part)) throw new Error(`unreadable key step: "${step}" (unknown modifier "${part}")`);
    if (part === "ControlOrMeta") out.ctrlOrMeta = true;
    else if (part === "Shift") out.shift = true;
    else out.alt = true;
  }
  return out;
}

const isLetter = (key: string): boolean => key.length === 1 && key.toLowerCase() !== key.toUpperCase();

/**
 * Whether the event is this step. Rules, so that no shortcut fires by accident:
 *  - Control and Command are the same modifier (ControlOrMeta), and they must match the step exactly;
 *  - Alt must match exactly (on a Mac it types other characters);
 *  - a letter matches either case (Caps Lock), but not with Shift held unless the step says Shift;
 *  - a punctuation key is compared by what it types, so `?` matches whatever Shift the layout needs for it;
 *  - a named key (Escape, Enter, Tab, an arrow) is compared by name and takes Shift only if the step says so.
 */
export function matchesStep(step: KeyStep, event: KeyLike): boolean {
  if (step.ctrlOrMeta !== (event.ctrlKey || event.metaKey)) return false;
  if (step.alt !== event.altKey) return false;
  if (step.key.length === 1 && !isLetter(step.key)) {
    return event.key === step.key && (!step.shift || event.shiftKey);
  }
  if (step.shift !== event.shiftKey) return false;
  if (isLetter(step.key)) return event.key.toLowerCase() === step.key.toLowerCase();
  return event.key === step.key;
}
