import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HIGHLIGHT_MS, useRecentlyChanged } from "./use-recently-changed";

type Row = { id: string; text: string };
const keyOf = (r: Row) => r.id;
const signatureOf = (r: Row) => r.text;
const setup = (initial: Row[]) => renderHook(({ rows }) => useRecentlyChanged(rows, keyOf, signatureOf), { initialProps: { rows: initial } });

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("useRecentlyChanged", () => {
  it("marks nothing for the first list", () => {
    const { result } = setup([{ id: "a", text: "x" }]);
    expect([...result.current]).toEqual([]);
  });

  it("marks a row that changed in place, and only that row", () => {
    const { result, rerender } = setup([{ id: "a", text: "x" }, { id: "b", text: "y" }]);
    rerender({ rows: [{ id: "a", text: "x2" }, { id: "b", text: "y" }] });
    expect([...result.current]).toEqual(["a"]);
  });

  it("does not mark a row that arrived or one that did not change", () => {
    const { result, rerender } = setup([{ id: "a", text: "x" }]);
    rerender({ rows: [{ id: "a", text: "x" }, { id: "n", text: "new" }] });
    expect([...result.current]).toEqual([]);
  });

  it("lets the mark go after a few seconds, and starts the count again when the row changes again", () => {
    const { result, rerender } = setup([{ id: "a", text: "x" }]);
    rerender({ rows: [{ id: "a", text: "x2" }] });
    act(() => {
      vi.advanceTimersByTime(HIGHLIGHT_MS - 100);
    });
    rerender({ rows: [{ id: "a", text: "x3" }] });
    act(() => {
      vi.advanceTimersByTime(HIGHLIGHT_MS - 100);
    });
    expect([...result.current]).toEqual(["a"]);
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect([...result.current]).toEqual([]);
  });

  it("leaves no timer behind when the screen goes", () => {
    const { rerender, unmount } = setup([{ id: "a", text: "x" }]);
    rerender({ rows: [{ id: "a", text: "x2" }] });
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
