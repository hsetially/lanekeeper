import { describe, expect, it, vi } from "vitest";
import { isChunkLoadError, retryImport } from "./retry-import";

// A dynamic import that cannot fetch its chunk is what a stale tab sees after a deploy removed the old files, and what a
// network blip looks like. Both have the same message, so the retry is bounded and short; the screen then says a new
// version may be available (src/components/app/NewVersion.tsx). A module that loaded and then threw is not retried.

const chunkError = () => new TypeError("Failed to fetch dynamically imported module: https://hub.example/assets/Files-abc123.js");

describe("isChunkLoadError", () => {
  it("knows the wording of each browser and of the bundler", () => {
    for (const message of [
      "Failed to fetch dynamically imported module: https://x/assets/a.js",
      "error loading dynamically imported module: https://x/assets/a.js",
      "Importing a module script failed.",
      "Unable to preload CSS for /assets/a.css",
      "Loading chunk 12 failed.",
      "Loading CSS chunk 3 failed.",
    ]) {
      expect(isChunkLoadError(new TypeError(message)), message).toBe(true);
    }
  });

  it("does not take any other error for one", () => {
    expect(isChunkLoadError(new Error("Cannot read properties of undefined"))).toBe(false);
    expect(isChunkLoadError(new SyntaxError("Unexpected token"))).toBe(false);
    expect(isChunkLoadError("Failed to fetch dynamically imported module")).toBe(false);
    expect(isChunkLoadError(undefined)).toBe(false);
    expect(isChunkLoadError(null)).toBe(false);
  });
});

describe("retryImport", () => {
  const fast = { sleep: () => Promise.resolve(), random: () => 0.5 };

  it("returns the module when the first try works, and tries once", async () => {
    const load = vi.fn(() => Promise.resolve({ ok: 1 }));
    await expect(retryImport(load, fast)).resolves.toEqual({ ok: 1 });
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("tries again after a chunk error, and returns what a later try gives", async () => {
    const load = vi.fn().mockRejectedValueOnce(chunkError()).mockResolvedValueOnce({ ok: 2 });
    await expect(retryImport(load, fast)).resolves.toEqual({ ok: 2 });
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("gives up after two retries, three tries in all, and throws the error it last saw", async () => {
    const last = chunkError();
    const load = vi.fn().mockRejectedValueOnce(chunkError()).mockRejectedValueOnce(chunkError()).mockRejectedValue(last);
    await expect(retryImport(load, fast)).rejects.toBe(last);
    expect(load).toHaveBeenCalledTimes(3);
  });

  it("does not retry an error that is not a failed fetch: the module loaded and broke", async () => {
    const load = vi.fn().mockRejectedValue(new Error("boom"));
    await expect(retryImport(load, fast)).rejects.toThrow("boom");
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("waits between tries, longer each time, with jitter, and never longer than a second", async () => {
    const waits: number[] = [];
    const load = vi.fn().mockRejectedValue(chunkError());
    await expect(retryImport(load, { sleep: (ms) => { waits.push(ms); return Promise.resolve(); }, random: () => 1 })).rejects.toThrow();
    expect(waits).toHaveLength(2);
    expect(waits[1] ?? 0).toBeGreaterThan(waits[0] ?? 0);
    expect(Math.max(...waits)).toBeLessThanOrEqual(1000);
    const low: number[] = [];
    await expect(retryImport(load, { sleep: (ms) => { low.push(ms); return Promise.resolve(); }, random: () => 0 })).rejects.toThrow();
    expect(Math.min(...low)).toBeGreaterThanOrEqual(0);
    expect(low[0]).toBeLessThan(waits[0] ?? 0);
  });
});
