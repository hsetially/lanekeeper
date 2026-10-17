import { describe, expect, it } from "vitest";
import { forAll } from "@/test/prng";
import { BACKOFF_CAP_MS, BACKOFF_MIN_MS, backoffCeilingMs, backoffDelayMs, MAX_FLOOR_MS } from "./backoff";

// Bounded exponential backoff with full jitter (rule 5). The ladder is 1, 2, 4, 8, 15 s and stays at 15 s; each delay is drawn
// from the whole range below the step, so clients that lost the stream together (a deploy) do not come back together.

describe("the ladder", () => {
  it("is 1, 2, 4, 8, 15 seconds and then stays at 15", () => {
    expect([0, 1, 2, 3, 4, 5, 6, 50].map(backoffCeilingMs)).toEqual([1000, 2000, 4000, 8000, 15_000, 15_000, 15_000, 15_000]);
  });

  it("is bounded for any attempt, including one that is not a count", () => {
    for (const attempt of [Number.MAX_SAFE_INTEGER, 10_000, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      const ceiling = backoffCeilingMs(attempt);
      expect(ceiling).toBeGreaterThanOrEqual(1000);
      expect(ceiling).toBeLessThanOrEqual(BACKOFF_CAP_MS);
    }
  });
});

describe("full jitter", () => {
  it("draws from the whole range: the lowest draw is the minimum, the highest is the step", () => {
    expect(backoffDelayMs(2, { random: () => 0 })).toBe(BACKOFF_MIN_MS);
    expect(backoffDelayMs(2, { random: () => 0.999_999_9 })).toBe(4000);
    expect(backoffDelayMs(9, { random: () => 0.999_999_9 })).toBe(BACKOFF_CAP_MS);
  });

  it("never returns less than the minimum, so a run of low draws cannot become a busy loop", () => {
    forAll(10_000, (rng) => ({ attempt: rng.int(12), r: rng.next() }), ({ attempt, r }) => {
      const delay = backoffDelayMs(attempt, { random: () => r });
      expect(delay).toBeGreaterThanOrEqual(BACKOFF_MIN_MS);
      expect(delay).toBeLessThanOrEqual(backoffCeilingMs(attempt));
      expect(Number.isInteger(delay)).toBe(true);
    });
  });

  it("spreads: over many draws the delays cover the range and average near its middle", () => {
    let seed = 12345;
    const random = () => {
      seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
      return seed / 4294967296;
    };
    const draws = Array.from({ length: 5000 }, () => backoffDelayMs(3, { random }));
    const mean = draws.reduce((a, b) => a + b, 0) / draws.length;
    expect(Math.min(...draws)).toBeLessThan(1000);
    expect(Math.max(...draws)).toBeGreaterThan(7000);
    expect(mean).toBeGreaterThan(3500);
    expect(mean).toBeLessThan(4700);
  });
});

describe("a floor (the server's retry hint after a clean end, Retry-After on a 429)", () => {
  it("never waits less than the floor, and jitters above it", () => {
    expect(backoffDelayMs(0, { floorMs: 3000, random: () => 0 })).toBe(3000);
    expect(backoffDelayMs(0, { floorMs: 3000, random: () => 0.999_999_9 })).toBe(6000);
    forAll(10_000, (rng) => ({ attempt: rng.int(8), floor: rng.int(20_000), r: rng.next() }), ({ attempt, floor, r }) => {
      const delay = backoffDelayMs(attempt, { floorMs: floor, random: () => r });
      expect(delay).toBeGreaterThanOrEqual(Math.max(BACKOFF_MIN_MS, floor));
      expect(delay).toBeLessThanOrEqual(Math.max(backoffCeilingMs(attempt), 2 * floor));
    });
  });

  it("bounds the floor itself: a server that says to wait an hour is waited for at most a minute", () => {
    expect(MAX_FLOOR_MS).toBe(60_000);
    expect(backoffDelayMs(0, { floorMs: 3_600_000, random: () => 0 })).toBe(MAX_FLOOR_MS);
    expect(backoffDelayMs(0, { floorMs: 3_600_000, random: () => 0.999_999_9 })).toBeLessThanOrEqual(2 * MAX_FLOOR_MS);
  });

  it("ignores a floor that is not a usable number", () => {
    for (const floorMs of [Number.NaN, -5, Number.NEGATIVE_INFINITY, 0]) {
      expect(backoffDelayMs(1, { floorMs, random: () => 0.999_999_9 })).toBe(2000);
    }
  });
});
