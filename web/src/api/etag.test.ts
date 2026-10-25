import { describe, expect, it } from "vitest";
import { EtagCache } from "./etag";

describe("EtagCache", () => {
  it("returns what was stored for a key", () => {
    const cache = new EtagCache();
    cache.set("/api/v1/a", '"v1"', '{"a":1}');
    expect(cache.get("/api/v1/a")).toEqual({ etag: '"v1"', text: '{"a":1}' });
    expect(cache.get("/api/v1/b")).toBeUndefined();
  });

  it("replaces an entry under the same key", () => {
    const cache = new EtagCache();
    cache.set("/k", '"v1"', "one");
    cache.set("/k", '"v2"', "two");
    expect(cache.get("/k")).toEqual({ etag: '"v2"', text: "two" });
    expect(cache.size).toBe(1);
  });

  it("keeps at most the entry limit and drops the least recently used first", () => {
    const cache = new EtagCache({ maxEntries: 3, maxBytes: 1_000_000 });
    cache.set("a", '"1"', "a");
    cache.set("b", '"1"', "b");
    cache.set("c", '"1"', "c");
    cache.get("a"); // a is now the most recent
    cache.set("d", '"1"', "d");
    expect(cache.size).toBe(3);
    expect(cache.get("b")).toBeUndefined();
    expect(cache.get("a")).toBeDefined();
    expect(cache.get("d")).toBeDefined();
  });

  it("keeps at most the byte budget", () => {
    const cache = new EtagCache({ maxEntries: 100, maxBytes: 1000 });
    for (let i = 0; i < 50; i++) cache.set(`k${i}`, '"1"', "x".repeat(100));
    expect(cache.bytes).toBeLessThanOrEqual(1000);
    expect(cache.get("k49")).toBeDefined();
    expect(cache.get("k0")).toBeUndefined();
  });

  it("does not keep one body that would take over the budget", () => {
    const cache = new EtagCache({ maxEntries: 100, maxBytes: 1000 });
    cache.set("small", '"1"', "x");
    cache.set("huge", '"1"', "x".repeat(900));
    expect(cache.get("huge")).toBeUndefined();
    expect(cache.get("small")).toBeDefined();
  });

  it("clears everything, as sign-out and a 401 do", () => {
    const cache = new EtagCache();
    cache.set("a", '"1"', "a");
    cache.clear();
    expect(cache.size).toBe(0);
    expect(cache.bytes).toBe(0);
    expect(cache.get("a")).toBeUndefined();
  });

  it("has the documented default limits: 200 entries and 8 MiB", () => {
    const cache = new EtagCache();
    expect(cache.limits).toEqual({ maxEntries: 200, maxBytes: 8 * 1024 * 1024 });
  });
});
