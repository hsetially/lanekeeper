import { describe, expect, it, vi } from "vitest";
import { MAX_TOASTS, ToastStore, defaultDuration } from "./store";

// The toast store holds what the toaster shows. Bounded (rule 5), one toast per key (a scan that sees the same file twice is
// one notice), and a toast of an error stays until a person dismisses it.

describe("ToastStore", () => {
  it("adds a toast and tells subscribers, newest last", () => {
    const store = new ToastStore();
    const seen = vi.fn();
    store.subscribe(seen);
    const a = store.push({ title: "one" });
    const b = store.push({ title: "two", kind: "success" });
    expect(store.getSnapshot().map((t) => [t.id, t.title, t.kind])).toEqual([
      [a, "one", "info"],
      [b, "two", "success"],
    ]);
    expect(seen).toHaveBeenCalledTimes(2);
  });

  it("keeps at most MAX_TOASTS and drops the oldest", () => {
    const store = new ToastStore();
    for (let i = 0; i < MAX_TOASTS + 3; i++) store.push({ title: `t${i}` });
    const titles = store.getSnapshot().map((t) => t.title);
    expect(titles).toHaveLength(MAX_TOASTS);
    expect(titles[0]).toBe("t3");
    expect(titles.at(-1)).toBe(`t${MAX_TOASTS + 2}`);
  });

  it("replaces the toast with the same key instead of adding another, and moves it to the end", () => {
    const store = new ToastStore();
    store.push({ title: "a", key: "x" });
    store.push({ title: "b" });
    const again = store.push({ title: "a, again", key: "x" });
    expect(store.getSnapshot().map((t) => t.title)).toEqual(["b", "a, again"]);
    expect(store.getSnapshot().at(-1)?.id).toBe(again);
  });

  it("dismisses one toast, ignores an id that is gone, and clears all", () => {
    const store = new ToastStore();
    const a = store.push({ title: "a" });
    store.push({ title: "b" });
    store.dismiss(a);
    store.dismiss(a);
    expect(store.getSnapshot().map((t) => t.title)).toEqual(["b"]);
    store.clear();
    expect(store.getSnapshot()).toEqual([]);
  });

  it("returns the same snapshot until something changes, so a subscriber does not re-render for nothing", () => {
    const store = new ToastStore();
    store.push({ title: "a" });
    const first = store.getSnapshot();
    expect(store.getSnapshot()).toBe(first);
    store.dismiss(9999);
    expect(store.getSnapshot()).toBe(first);
  });

  it("unsubscribes", () => {
    const store = new ToastStore();
    const seen = vi.fn();
    const off = store.subscribe(seen);
    off();
    store.push({ title: "a" });
    expect(seen).not.toHaveBeenCalled();
  });

  it("gives an error no timeout and the others a few seconds, unless the caller says", () => {
    expect(defaultDuration("danger")).toBeNull();
    expect(defaultDuration("info")).toBeGreaterThanOrEqual(5000);
    const store = new ToastStore();
    store.push({ title: "err", kind: "danger" });
    store.push({ title: "info" });
    store.push({ title: "pinned", durationMs: null });
    store.push({ title: "quick", durationMs: 1000 });
    expect(store.getSnapshot().map((t) => t.durationMs)).toEqual([null, defaultDuration("info"), null, 1000]);
  });
});
