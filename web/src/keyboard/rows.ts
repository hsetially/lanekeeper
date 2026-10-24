// The shortcut rows the app knows: the handoff's keymap (the reviewed copy in ./keymap.ts, compared with the extracted
// src/handoff/keymap.generated.ts by keymap.test.ts)
// and the reviewed list of screen-local hints (H-11). A row has an id, a scope and one or more bindings; a screen binds a
// handler to a row id with `useShortcut(id, handler)` and never invents a key. A key that is in neither list does not exist:
// `rows.test.ts` reads every `useShortcut` call in the source and fails on an id that is in neither.

import { KEYMAP_DATA } from "./keymap";
import { parseStep, type KeyStep } from "./keys";

export type KeymapRowId = (typeof KEYMAP_DATA)[number]["id"];

export type KeyBinding =
  | { readonly steps: readonly string[]; readonly when?: string; readonly action: string }
  /** Type-ahead has no fixed key: a tree jumps to the next row that starts with what was typed. The tree handles it itself. */
  | { readonly kind: "typeahead"; readonly action: string };

export type KeymapRow = {
  readonly id: string;
  readonly scope: string;
  /** How the handoff writes it ("g then s / c / f / i / a"). */
  readonly keys: string;
  readonly action: string;
  readonly bindings: readonly KeyBinding[];
  /** A row with no key yet: why. */
  readonly skip?: string;
};

/**
 * Hints the pages show that the handoff's keys table does not list (H-11), found by reading the `<kbd>` of every page file.
 * Each is reviewed here, with where it comes from. A screen that needs another one asks for it to be added: it is a change to
 * the keyboard map, which is the design owner's.
 */
export const LOCAL_ROWS = [
  {
    id: "local.filter.focus",
    scope: "Screens with a filter field",
    keys: "/",
    action: "Focus the filter field (Swimlanes, Swimlane detail, Compare, Feature status)",
    bindings: [{ steps: ["/"], action: "Focus the filter field" }],
  },
  {
    id: "local.detail.filter-files",
    scope: "Swimlane detail",
    keys: "f",
    action: "Filter files…",
    bindings: [{ steps: ["f"], action: "Filter files" }],
  },
] as const satisfies readonly KeymapRow[];

export type LocalRowId = (typeof LOCAL_ROWS)[number]["id"];
export type ShortcutId = KeymapRowId | LocalRowId;

export const KEYMAP_ROWS: readonly KeymapRow[] = KEYMAP_DATA;
export const ALL_ROWS: readonly KeymapRow[] = [...KEYMAP_ROWS, ...LOCAL_ROWS];

const BY_ID = new Map<string, KeymapRow>(ALL_ROWS.map((row) => [row.id, row]));

export function rowById(id: string): KeymapRow | undefined {
  return BY_ID.get(id);
}

export function isShortcutId(id: string): id is ShortcutId {
  return BY_ID.has(id);
}

/** One binding as steps the matcher can compare. A type-ahead binding has none. */
export type ParsedBinding = { readonly index: number; readonly steps: readonly KeyStep[]; readonly when: string | undefined; readonly action: string };

const parsed = new Map<string, readonly ParsedBinding[]>();

/** The key bindings of a row, parsed once. A row with only type-ahead or no keys yields none. */
export function bindingsOf(id: string): readonly ParsedBinding[] {
  const cached = parsed.get(id);
  if (cached) return cached;
  const row = BY_ID.get(id);
  const out: ParsedBinding[] = [];
  row?.bindings.forEach((binding, index) => {
    if ("steps" in binding) out.push({ index, steps: binding.steps.map(parseStep), when: binding.when, action: binding.action });
  });
  parsed.set(id, out);
  return out;
}
