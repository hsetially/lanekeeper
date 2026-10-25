// Preferences: the ONLY module allowed to use web storage (the lint names this one path). It stores layout choices and
// nothing else (S2, S10): whether the sidebar is collapsed, which table columns are shown, how wide a split pane is. Keys
// come from a whitelist, values are validated on the way in and on the way out, and a value is a boolean, a number or a
// short list of identifiers, so it cannot hold file content, a name or a token even by mistake. API data is never stored.
// The current swimlane is deliberately not here: it lives in memory only.
//
// Storage can be missing, full or blocked (a private window, a policy); then the values live in memory for the tab.

export type PrefsStorage = Pick<Storage, "getItem" | "setItem" | "removeItem" | "key" | "length">;

const PREFIX = "lk.pref.";
const ID = /^[a-z][a-z0-9-]{0,31}$/;
const MAX_VALUE_BYTES = 2048;
const MAX_COLUMNS = 32;

type Scalar = { kind: "scalar"; fallback: boolean; valid: (v: unknown) => v is boolean };
type Keyed<T> = { kind: "keyed"; valid: (v: unknown) => v is T };

const isBoolean = (v: unknown): v is boolean => typeof v === "boolean";
const isColumns = (v: unknown): v is string[] => Array.isArray(v) && v.length <= MAX_COLUMNS && v.every((c) => typeof c === "string" && ID.test(c));
const isWidth = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0 && v <= 10_000;

const SPEC = {
  sidebarCollapsed: { kind: "scalar", fallback: false, valid: isBoolean } satisfies Scalar,
  tableColumns: { kind: "keyed", valid: isColumns } satisfies Keyed<string[]>,
  splitWidth: { kind: "keyed", valid: isWidth } satisfies Keyed<number>,
} as const;

type Spec = typeof SPEC;
type ValueOf<K extends keyof Spec> = Spec[K] extends { valid: (v: unknown) => v is infer T } ? T : never;
type ScalarKeys = { [K in keyof Spec]: Spec[K] extends { kind: "scalar" } ? K : never }[keyof Spec];
type KeyedKeys = { [K in keyof Spec]: Spec[K] extends { kind: "keyed" } ? K : never }[keyof Spec];

function resolve(): PrefsStorage | undefined {
  return typeof window === "undefined" ? undefined : window.localStorage;
}

export function createPrefs(storageOf: () => PrefsStorage | undefined = resolve) {
  const memory = new Map<string, string>();

  const read = (key: string): string | null => {
    try {
      const stored = storageOf()?.getItem(key);
      if (stored !== undefined && stored !== null) return stored;
    } catch {
      // Blocked storage reads as empty.
    }
    return memory.get(key) ?? null;
  };
  const write = (key: string, text: string): void => {
    memory.set(key, text);
    try {
      storageOf()?.setItem(key, text);
    } catch {
      // Full or blocked: the memory copy is the value for this tab.
    }
  };

  function keyFor(name: string, id?: string): string | undefined {
    if (id === undefined) return `${PREFIX}${name}`;
    return ID.test(id) ? `${PREFIX}${name}.${id}` : undefined;
  }

  function load(key: string | undefined, valid: (v: unknown) => boolean): unknown {
    if (key === undefined) return undefined;
    const text = read(key);
    if (text === null || text.length > MAX_VALUE_BYTES) return undefined;
    try {
      const value: unknown = JSON.parse(text);
      return valid(value) ? value : undefined;
    } catch {
      return undefined;
    }
  }

  function get(name: ScalarKeys): boolean;
  function get<K extends KeyedKeys>(name: K, id: string): ValueOf<K> | undefined;
  function get(name: keyof Spec, id?: string): unknown {
    const spec = SPEC[name];
    if (spec.kind === "scalar") return load(keyFor(name), spec.valid) ?? spec.fallback;
    return load(keyFor(name, id), spec.valid);
  }

  function set(name: ScalarKeys, value: boolean): void;
  function set<K extends KeyedKeys>(name: K, id: string, value: ValueOf<K>): void;
  function set(name: keyof Spec, ...rest: unknown[]): void {
    const spec = SPEC[name];
    const id = spec.kind === "keyed" ? rest[0] : undefined;
    const value = spec.kind === "keyed" ? rest[1] : rest[0];
    if (spec.kind === "keyed" && typeof id !== "string") return;
    if (!spec.valid(value)) return;
    const key = keyFor(name, id as string | undefined);
    if (key === undefined) return;
    const text = JSON.stringify(value);
    if (text.length > MAX_VALUE_BYTES) return;
    write(key, text);
  }

  /** Forget every preference of this app (and nothing else in storage). */
  function clear(): void {
    memory.clear();
    try {
      const storage = storageOf();
      if (!storage) return;
      const own: string[] = [];
      for (let i = 0; i < storage.length; i++) {
        const key = storage.key(i);
        if (key?.startsWith(PREFIX)) own.push(key);
      }
      for (const key of own) storage.removeItem(key);
    } catch {
      // Nothing more can be done.
    }
  }

  return { get, set, clear };
}

export const prefs = createPrefs();
