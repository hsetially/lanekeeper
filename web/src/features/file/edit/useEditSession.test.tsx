import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useEditSession } from "./useEditSession";

// What editing a file remembers: the version it started from (its hash and text, taken when Edit was pressed and NEVER moved
// by a refresh, so that a save is always based on what the person saw), what they have typed, and whether it differs.

const HASH = "a".repeat(64);
const OTHER = "b".repeat(64);

describe("useEditSession", () => {
  it("is not editing until it starts", () => {
    const { result } = renderHook(() => useEditSession());
    expect(result.current.session).toBeNull();
    expect(result.current.dirty).toBe(false);
  });

  it("starts from the version the person saw, with the text unchanged", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    expect(result.current.session).toMatchObject({ base: { hash: HASH, text: "a: 1\n" }, text: "a: 1\n", rebasedFrom: null });
    expect(result.current.dirty).toBe(false);
  });

  it("is dirty once the text differs, and clean again when it is typed back", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    act(() => result.current.setText("a: 2\n"));
    expect(result.current.dirty).toBe(true);
    act(() => result.current.setText("a: 1\n"));
    expect(result.current.dirty).toBe(false);
  });

  it("bases the next save on a newer version, keeping what the person typed, and remembers what it was based on", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    act(() => result.current.setText("a: 2\n"));
    act(() => result.current.rebase(OTHER));
    expect(result.current.session).toMatchObject({ base: { hash: OTHER, text: "a: 1\n" }, text: "a: 2\n", rebasedFrom: HASH });
    expect(result.current.dirty).toBe(true);
  });

  it("can be based on a file that is gone", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    act(() => result.current.rebase(null));
    expect(result.current.session?.base.hash).toBeNull();
  });

  it("ends, and forgets everything", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    act(() => result.current.setText("changed"));
    act(() => result.current.end());
    expect(result.current.session).toBeNull();
    expect(result.current.dirty).toBe(false);
  });

  it("ignores typing when it is not editing", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.setText("x"));
    expect(result.current.session).toBeNull();
  });

  it("registers unsaved work with the dirty registry while there is some, and offers the draft to copy", async () => {
    const { hasDirty, collectDrafts, clearDirtyForTests } = await import("@/auth/dirty");
    clearDirtyForTests();
    const { result, unmount } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    expect(hasDirty()).toBe(false);
    act(() => result.current.setText("a: 2\n"));
    expect(hasDirty()).toBe(true);
    expect(collectDrafts()).toBe("a: 2\n");
    unmount();
    expect(hasDirty()).toBe(false);
  });

  // A key can arrive between the editor's change and the render that follows it (React draws a state change on its own turn, and
  // a handler bound with useShortcut sees the render it was made in): Escape right after a letter must see the letter, or the edit is
  // thrown away without asking. Found by the keyboard run, one time in sixteen, when the test typed and pressed Escape within a few
  // milliseconds. `isDirty` answers from what was set, not from the last render.
  it("says it is dirty at once, before the render that follows the change, to a handler that was made in an earlier render", () => {
    const { result } = renderHook(() => useEditSession());
    act(() => result.current.start({ hash: HASH, text: "a: 1\n" }));
    const staleRender = result.current;
    let during: { dirty: boolean; isDirty: boolean; typedBack: boolean } | undefined;
    act(() => {
      staleRender.setText("a: 2\n");
      const isDirty = staleRender.isDirty();
      const dirty = staleRender.dirty;
      staleRender.setText("a: 1\n");
      during = { dirty, isDirty, typedBack: staleRender.isDirty() };
    });
    // The render of the change has not happened, so the old closure says clean; the question asked of it now says dirty.
    expect(during).toEqual({ dirty: false, isDirty: true, typedBack: false });
  });

  it("answers for a start and an end in the same turn too", () => {
    const { result } = renderHook(() => useEditSession());
    const first = result.current;
    act(() => {
      first.start({ hash: HASH, text: "a" });
      first.setText("b");
      expect(first.isDirty()).toBe(true);
      first.end();
      expect(first.isDirty()).toBe(false);
    });
  });
});
