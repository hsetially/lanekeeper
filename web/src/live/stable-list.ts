import { useCallback, useEffect, useState } from "react";

// The stable-view rule of the handoff, for a list that live events refresh. A row that changes IN PLACE (same keys, same
// order) updates in place. A row that appears, goes or moves would shift the list under the reader's pointer and keyboard
// focus, so the list is held still and the change is counted ("3 updates"); `apply` shows it when the reader asks.

/** True when both lists have the same keys in the same order, whatever the rows hold. */
export function sameKeysInOrder<T>(a: readonly T[], b: readonly T[], keyOf: (item: T) => string): boolean {
  return a.length === b.length && a.every((item, i) => b[i] !== undefined && keyOf(item) === keyOf(b[i] as T));
}

/** The rows that came plus the rows that went, plus one when the rows both have are in another order. 0 for the same keys. */
export function countChanges<T>(shown: readonly T[], latest: readonly T[], keyOf: (item: T) => string): number {
  const before = shown.map(keyOf);
  const after = latest.map(keyOf);
  const beforeSet = new Set(before);
  const afterSet = new Set(after);
  const came = after.filter((k) => !beforeSet.has(k)).length;
  const went = before.filter((k) => !afterSet.has(k)).length;
  const common = before.filter((k) => afterSet.has(k));
  const commonAfter = after.filter((k) => beforeSet.has(k));
  const moved = common.some((k, i) => k !== commonAfter[i]) ? 1 : 0;
  return came + went + moved;
}

export type StableList<T> = {
  /** The list to draw. */
  readonly shown: readonly T[];
  /** How many changes are held back. 0 when the list on screen is current. */
  readonly pending: number;
  /** Show the latest list. */
  readonly apply: () => void;
};

export function useStableList<T>(items: readonly T[], keyOf: (item: T) => string): StableList<T> {
  const [held, setHeld] = useState<readonly T[]>(items);
  const same = sameKeysInOrder(held, items, keyOf);
  // In place: the rows are the latest ones. Otherwise the held list is what is drawn, and the difference is counted.
  useEffect(() => {
    if (same && held !== items) setHeld(items);
  }, [same, held, items]);
  const apply = useCallback(() => {
    setHeld(items);
  }, [items]);
  return { shown: same ? items : held, pending: same ? 0 : countChanges(held, items, keyOf), apply };
}
