import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { within } from "./bounded.mjs";

// `within` is the one way the Lighthouse runner waits for something that can take forever (a browser, the debugging protocol, a
// whole run): it either hands back the answer or fails, at the bound, with a message that names what was waited for. The timer
// it uses never outlives the wait, so a run that finished is not held open by the bound of the last wait.
beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("within", () => {
  it("hands back the answer, and leaves no timer behind", async () => {
    const answer = within(Promise.resolve("done"), 1000, "never");
    await expect(answer).resolves.toBe("done");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("hands back the failure of the work as it is, and leaves no timer behind", async () => {
    const failure = new Error("the work failed");
    const answer = within(Promise.reject(failure), 1000, "never");
    await expect(answer).rejects.toBe(failure);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("fails with the message at the bound when the work has not finished", async () => {
    const answer = within(new Promise(() => undefined), 1000, "Lighthouse did not answer in 1000 ms");
    const settled = answer.then(
      () => "answered",
      (/** @type {unknown} */ e) => (e instanceof Error ? e.message : String(e)),
    );
    await vi.advanceTimersByTimeAsync(999);
    expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersByTimeAsync(1);
    await expect(settled).resolves.toBe("Lighthouse did not answer in 1000 ms");
  });

  it("asks a message that is a function for its text only when the bound is reached", async () => {
    let asked = 0;
    const answer = within(new Promise(() => undefined), 50, () => `at the bound: ${++asked}`);
    const settled = answer.then(
      () => "answered",
      (/** @type {unknown} */ e) => (e instanceof Error ? e.message : String(e)),
    );
    await vi.advanceTimersByTimeAsync(49);
    expect(asked).toBe(0);
    await vi.advanceTimersByTimeAsync(1);
    await expect(settled).resolves.toBe("at the bound: 1");
  });

  it("does not fail a second time, or answer late, when the work finishes after the bound", async () => {
    /** @type {(v: string) => void} */
    let finish = () => undefined;
    const work = new Promise((resolve) => {
      finish = resolve;
    });
    const answer = within(work, 100, "too slow");
    const settled = answer.then(
      () => "answered",
      (/** @type {unknown} */ e) => (e instanceof Error ? e.message : String(e)),
    );
    await vi.advanceTimersByTimeAsync(100);
    await expect(settled).resolves.toBe("too slow");
    finish("late");
    await vi.advanceTimersByTimeAsync(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});
