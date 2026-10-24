import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// A drawer takes focus when it opens, and again once the navigation that opened it has finished (the shell puts focus on the page's
// h1 then, which would take it from the drawer). The audit and approvals drawers each carried the same hook.

const subscribers: Array<() => void> = [];
const off = vi.fn();
vi.mock("@tanstack/react-router", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-router")>()),
  useRouter: () => ({
    subscribe: (_event: string, run: () => void) => {
      subscribers.push(run);
      return off;
    },
  }),
}));

const { useFocusOnOpen } = await import("./focus");

beforeEach(() => {
  vi.useFakeTimers();
  subscribers.length = 0;
  off.mockClear();
  document.body.innerHTML = '<h2 id="h" tabindex="-1">Event</h2><h1 id="page" tabindex="-1">Page</h1>';
});
afterEach(() => {
  vi.useRealTimers();
});

const heading = () => document.getElementById("h") as HTMLElement;

describe("useFocusOnOpen", () => {
  it("focuses the element when the drawer opens, without scrolling", () => {
    const focus = vi.spyOn(heading(), "focus");
    renderHook(() => {
      useFocusOnOpen({ current: heading() });
    });
    expect(document.activeElement).toBe(heading());
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it("takes focus back when the navigation that opened the drawer finishes rendering", () => {
    renderHook(() => {
      useFocusOnOpen({ current: heading() });
    });
    document.getElementById("page")?.focus();
    expect(document.activeElement?.id).toBe("page");
    subscribers[0]?.();
    expect(document.activeElement).toBe(heading());
  });

  it("stops listening after 1.5 s, and when the drawer closes", () => {
    const { unmount } = renderHook(() => {
      useFocusOnOpen({ current: heading() });
    });
    expect(off).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1499);
    expect(off).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(off).toHaveBeenCalledTimes(1);
    unmount();
    expect(off).toHaveBeenCalledTimes(2);
  });

  it("is written once", async () => {
    const { productSources } = await import("@/test/source-files");
    const holders = productSources().filter(({ text }) => /function useFocusOnOpen\b/.test(text)).map((f) => f.path);
    expect(holders).toEqual(["src/features/shell/focus.ts"]);
  });
});
