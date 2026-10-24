// Key caps for the shortcuts sheet, the palette's hints and tooltips, written the way the handoff draws them: `⌘` and `K` in
// a chord (a capital letter), `g` then `s` in a sequence (lower case), `Esc`, `↵`, the arrows. They are display only; the
// matching is in keys.ts.

import { bindingsOf, rowById } from "./rows";

const NAMED: Record<string, string> = {
  Escape: "Esc",
  Enter: "↵",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Backspace: "⌫",
  Space: "Space",
  Tab: "Tab",
  Home: "Home",
  End: "End",
};

/** One step as key caps: `ControlOrMeta+k` is `["⌘", "K"]`, `g` is `["g"]`, `Shift+Tab` is `["⇧", "Tab"]`. */
export function stepCaps(step: string): string[] {
  const parts = step.split("+").filter((p) => p !== "");
  const key = parts.pop() ?? step;
  const modifiers = parts.map((m) => (m === "ControlOrMeta" ? "⌘" : m === "Shift" ? "⇧" : m === "Alt" ? "⌥" : m));
  const shown = NAMED[key] ?? (modifiers.length > 0 && key.length === 1 ? key.toUpperCase() : key);
  return [...modifiers, shown];
}

/** The caps of a binding: a sequence is its steps joined by the word `then`, which a screen draws as text, not as a key. */
export function bindingCaps(steps: readonly string[]): string[] {
  return steps.flatMap((step, i) => (i === 0 ? stepCaps(step) : ["then", ...stepCaps(step)]));
}

/** The caps of one binding of a row (the first when `index` is left out). Empty when the row has no key. */
export function rowCaps(id: string, index = 0): string[] {
  const row = rowById(id);
  const binding = row?.bindings[index];
  if (!binding || !("steps" in binding)) return [];
  return bindingCaps(binding.steps);
}

/** True when the row can be bound (it has at least one key). */
export function hasKey(id: string): boolean {
  return bindingsOf(id).length > 0;
}

/** `⌘K`, `g then s`: the caps as one plain string, for a title or an accessible name. */
export function capsText(caps: readonly string[]): string {
  return caps.join(" ").replace(/⌘ (?=\S)/g, "⌘").replace(/⇧ (?=\S)/g, "⇧").replace(/⌥ (?=\S)/g, "⌥");
}
