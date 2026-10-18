import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useTheme } from "./theme";

// The theme switch: Light, Dark, or follow the system. The choice is the `lk_theme` cookie (src/lib/prefs/theme.ts), applied
// as the `dark` class on <html>; "system" follows the operating system while the page is open.

let cookie = "";
let listeners: Array<(event: { matches: boolean }) => void> = [];
let systemDark = false;

beforeEach(() => {
  cookie = "";
  listeners = [];
  systemDark = false;
  document.documentElement.classList.remove("dark");
  Object.defineProperty(document, "cookie", { configurable: true, get: () => cookie, set: (v: string) => { cookie = v.split(";")[0] ?? ""; } });
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("dark") ? systemDark : false,
    media: query,
    addEventListener: (_: string, l: (e: { matches: boolean }) => void) => listeners.push(l),
    removeEventListener: (_: string, l: (e: { matches: boolean }) => void) => {
      listeners = listeners.filter((x) => x !== l);
    },
  }));
});
afterEach(() => {
  vi.unstubAllGlobals();
  // @ts-expect-error restore the jsdom accessor
  delete document.cookie;
});

describe("useTheme", () => {
  it("starts from the cookie, and from system when there is none", () => {
    expect(renderHook(() => useTheme()).result.current.choice).toBe("system");
    cookie = "lk_theme=dark";
    expect(renderHook(() => useTheme()).result.current.choice).toBe("dark");
  });

  it("sets the class and the cookie when the choice changes", () => {
    const { result } = renderHook(() => useTheme());
    act(() => {
      result.current.set("dark");
    });
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(cookie).toBe("lk_theme=dark");
    act(() => {
      result.current.set("light");
    });
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(cookie).toBe("lk_theme=light");
  });

  it("follows the system while the choice is system, and stops following when it is not", () => {
    const { result } = renderHook(() => useTheme());
    expect(listeners).toHaveLength(1);
    act(() => {
      listeners.forEach((l) => l({ matches: true }));
    });
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    act(() => {
      result.current.set("light");
    });
    expect(listeners).toHaveLength(0);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("removes its listener when the component goes", () => {
    const { unmount } = renderHook(() => useTheme());
    unmount();
    expect(listeners).toHaveLength(0);
  });
});
