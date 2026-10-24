import { afterEach, describe, expect, it, vi } from "vitest";
import { saveDataOn, scheduleIdle } from "./idle";

// The two small browser facts that the idle work of the app asks (the route preloads, the editor warm-up and the command palette's
// chunk): when is the browser idle, and did the person ask for less data. Each used to be written in two or three places.

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("scheduleIdle", () => {
  it("runs the work when the browser is idle, with the timeout it was given as the longest wait, and cancels it", () => {
    const request = vi.fn((_run: () => void, _options?: { timeout: number }) => 7);
    const cancel = vi.fn();
    vi.stubGlobal("requestIdleCallback", request);
    vi.stubGlobal("cancelIdleCallback", cancel);
    const run = vi.fn();
    const stop = scheduleIdle(run, { timeoutMs: 6000, fallbackMs: 1500 });
    expect(request).toHaveBeenCalledTimes(1);
    expect(request.mock.calls[0]?.[0]).toBe(run);
    expect(request.mock.calls[0]?.[1]).toEqual({ timeout: 6000 });
    stop();
    expect(cancel).toHaveBeenCalledWith(7);
  });

  it("waits fallbackMs on a timer where the browser has no idle callback, and cancels the timer", () => {
    vi.useFakeTimers();
    vi.stubGlobal("requestIdleCallback", undefined);
    const run = vi.fn();
    const stop = scheduleIdle(run, { timeoutMs: 5000, fallbackMs: 200 });
    vi.advanceTimersByTime(199);
    expect(run).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(run).toHaveBeenCalledTimes(1);

    const later = vi.fn();
    const stopLater = scheduleIdle(later, { timeoutMs: 5000, fallbackMs: 1000 });
    vi.advanceTimersByTime(500);
    stopLater();
    vi.advanceTimersByTime(5000);
    expect(later).not.toHaveBeenCalled();
    stop();
  });

  it("cancels cleanly where a browser has requestIdleCallback and no cancelIdleCallback", () => {
    vi.stubGlobal("requestIdleCallback", () => 1);
    vi.stubGlobal("cancelIdleCallback", undefined);
    expect(() => {
      scheduleIdle(() => undefined, { timeoutMs: 5000, fallbackMs: 200 })();
    }).not.toThrow();
  });
});

describe("saveDataOn", () => {
  const connection = (value: unknown) => vi.stubGlobal("navigator", { connection: value });

  it("is true only when the person asked the browser to save data", () => {
    connection({ saveData: true });
    expect(saveDataOn()).toBe(true);
    connection({ saveData: false });
    expect(saveDataOn()).toBe(false);
    connection({});
    expect(saveDataOn()).toBe(false);
    connection(undefined);
    expect(saveDataOn()).toBe(false);
    connection({ saveData: "true" });
    expect(saveDataOn()).toBe(false);
  });
});

describe("one definition", () => {
  it("is the only place that reads the connection or asks for an idle moment", async () => {
    const { productSources, withoutComments } = await import("@/test/source-files");
    const reads = productSources().filter(({ text }) => /\.connection\b/.test(withoutComments(text))).map((f) => f.path);
    expect(reads).toEqual(["src/lib/idle.ts"]);
    const idle = productSources().filter(({ text }) => /\brequestIdleCallback\b/.test(withoutComments(text))).map((f) => f.path);
    expect(idle).toEqual(["src/lib/idle.ts"]);
  });
});
