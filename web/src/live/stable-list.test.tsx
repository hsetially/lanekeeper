import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { countChanges, sameKeysInOrder, useStableList } from "./stable-list";

// The handoff's stable-view rule for a list that live events refresh: a row that changes in place updates in place; a row that
// appears, disappears or moves does NOT shift the list under the reader's pointer, it is counted ("3 updates") until they ask.

type Row = { id: string; text: string };
const row = (id: string, text = id): Row => ({ id, text });
const keyOf = (r: Row) => r.id;

describe("sameKeysInOrder and countChanges", () => {
  it("is the same list when the keys are the same in the same order, whatever the rows hold", () => {
    expect(sameKeysInOrder([row("a"), row("b")], [row("a", "x"), row("b", "y")], keyOf)).toBe(true);
    expect(sameKeysInOrder([row("a"), row("b")], [row("b"), row("a")], keyOf)).toBe(false);
    expect(sameKeysInOrder([row("a")], [row("a"), row("b")], keyOf)).toBe(false);
    expect(sameKeysInOrder([], [], keyOf)).toBe(true);
  });

  it("counts the rows that came, the rows that went, and one for a reorder", () => {
    expect(countChanges([row("a"), row("b")], [row("a"), row("b"), row("c")], keyOf)).toBe(1);
    expect(countChanges([row("a"), row("b")], [row("b")], keyOf)).toBe(1);
    expect(countChanges([row("a"), row("b")], [row("c"), row("d")], keyOf)).toBe(4);
    expect(countChanges([row("a"), row("b")], [row("b"), row("a")], keyOf)).toBe(1);
    expect(countChanges([row("a"), row("b")], [row("a"), row("b")], keyOf)).toBe(0);
  });
});

describe("useStableList with appendsApply (a list that pages)", () => {
  const setup = (initial: Row[]) => renderHook(({ items }) => useStableList(items, keyOf, { appendsApply: true }), { initialProps: { items: initial } });

  it("shows rows that are appended after the ones on screen, at once and with nothing pending: they are the next page", () => {
    const { result, rerender } = setup([row("a"), row("b")]);
    rerender({ items: [row("a"), row("b"), row("c"), row("d")] });
    expect(result.current.shown.map(keyOf)).toEqual(["a", "b", "c", "d"]);
    expect(result.current.pending).toBe(0);
  });

  it("still holds the list when a row arrives before the ones on screen, goes, or moves", () => {
    const { result, rerender } = setup([row("a"), row("b")]);
    rerender({ items: [row("c"), row("a"), row("b")] });
    expect(result.current.shown.map(keyOf)).toEqual(["a", "b"]);
    expect(result.current.pending).toBe(1);
    rerender({ items: [row("a"), row("c"), row("b")] });
    expect(result.current.shown.map(keyOf)).toEqual(["a", "b"]);
    rerender({ items: [row("b"), row("a"), row("c")] });
    expect(result.current.pending).toBeGreaterThan(0);
  });

  it("without the option an appended row is a change that is counted, as before", () => {
    const { result, rerender } = renderHook(({ items }) => useStableList(items, keyOf), { initialProps: { items: [row("a")] } });
    rerender({ items: [row("a"), row("b")] });
    expect(result.current.pending).toBe(1);
  });
});

describe("useStableList", () => {
  const setup = (initial: Row[]) => renderHook(({ items }) => useStableList(items, keyOf), { initialProps: { items: initial } });

  it("shows the list as it is at first, with nothing pending", () => {
    const { result } = setup([row("a"), row("b")]);
    expect(result.current.shown.map(keyOf)).toEqual(["a", "b"]);
    expect(result.current.pending).toBe(0);
  });

  it("updates a row in place when the keys and the order are the same", () => {
    const { result, rerender } = setup([row("a", "old"), row("b")]);
    rerender({ items: [row("a", "new"), row("b")] });
    expect(result.current.shown[0]?.text).toBe("new");
    expect(result.current.pending).toBe(0);
  });

  it("holds the list still when a row arrives, and counts it", () => {
    const { result, rerender } = setup([row("a"), row("b")]);
    rerender({ items: [row("c"), row("a"), row("b")] });
    expect(result.current.shown.map(keyOf)).toEqual(["a", "b"]);
    expect(result.current.pending).toBe(1);
  });

  it("holds it still when a row goes or the order changes, and counts what changed", () => {
    const { result, rerender } = setup([row("a"), row("b"), row("c")]);
    rerender({ items: [row("b"), row("c")] });
    expect(result.current.shown.map(keyOf)).toEqual(["a", "b", "c"]);
    expect(result.current.pending).toBe(1);
    rerender({ items: [row("c"), row("b"), row("d")] });
    expect(result.current.pending).toBe(3);
  });

  it("applies the latest list when asked, and then has nothing pending", () => {
    const { result, rerender } = setup([row("a"), row("b")]);
    rerender({ items: [row("c"), row("a"), row("b")] });
    act(() => result.current.apply());
    expect(result.current.shown.map(keyOf)).toEqual(["c", "a", "b"]);
    expect(result.current.pending).toBe(0);
  });

  it("clears what is pending when the list comes back to what is shown", () => {
    const { result, rerender } = setup([row("a"), row("b")]);
    rerender({ items: [row("a"), row("b"), row("c")] });
    expect(result.current.pending).toBe(1);
    rerender({ items: [row("a", "again"), row("b")] });
    expect(result.current.pending).toBe(0);
    expect(result.current.shown[0]?.text).toBe("again");
  });

  it("keeps counting against the held list as more changes arrive, not against the last one it saw", () => {
    const { result, rerender } = setup([row("a")]);
    rerender({ items: [row("a"), row("b")] });
    rerender({ items: [row("a"), row("b"), row("c")] });
    expect(result.current.shown.map(keyOf)).toEqual(["a"]);
    expect(result.current.pending).toBe(2);
  });

  it("shows an update that arrives for a held list's rows once the list is applied", () => {
    const { result, rerender } = setup([row("a", "1")]);
    rerender({ items: [row("b", "x"), row("a", "2")] });
    act(() => result.current.apply());
    expect(result.current.shown.find((r) => r.id === "a")?.text).toBe("2");
  });
});
