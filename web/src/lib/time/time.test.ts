import { describe, expect, it } from "vitest";
import { createTimeFormatter, formatClock, formatRelative, formatTime, systemClock } from "./index";

const T = Date.UTC(2026, 9, 17, 6, 42, 5, 123); // 2026-10-17 06:42:05.123 UTC
const sec = 1000;
const min = 60 * sec;
const hour = 60 * min;
const day = 24 * hour;

describe("formatTime (UTC, with the zone spelled out)", () => {
  it.each([
    [T, "2026-10-17 06:42 UTC"],
    [Date.UTC(2026, 0, 2, 3, 4, 59), "2026-01-02 03:04 UTC"],
    [Date.UTC(2026, 11, 31, 23, 59), "2026-12-31 23:59 UTC"],
    [0, "1970-01-01 00:00 UTC"],
  ])("%d is %s", (ms, text) => expect(formatTime(ms)).toBe(text));

  it("can show seconds", () => expect(formatTime(T, { seconds: true })).toBe("2026-10-17 06:42:05 UTC"));

  it("never throws and never prints Invalid Date", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -1e18, 1e18]) expect(formatTime(bad)).toBe("unknown time");
  });
});

describe("formatClock (the time of day, for a pill or a banner that says when the data is from)", () => {
  it("is the hour and minute in UTC with the zone spelled out", () => {
    expect(formatClock(T)).toBe("06:42 UTC");
    expect(formatClock(Date.UTC(2026, 0, 2, 23, 5))).toBe("23:05 UTC");
  });
  it("never throws and never prints Invalid Date", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -1, 1e18]) expect(formatClock(bad)).toBe("unknown time");
  });
});

describe("formatRelative", () => {
  const rel = (delta: number) => formatRelative(T - delta, T);
  it.each([
    [0, "just now"],
    [20 * sec, "just now"],
    [44 * sec, "just now"],
    [45 * sec, "1 min ago"],
    [89 * sec, "1 min ago"],
    [4 * min, "4 min ago"],
    [59 * min, "59 min ago"],
    [60 * min, "1 h ago"],
    [5 * hour + 20 * min, "5 h ago"],
    [23 * hour, "23 h ago"],
    [24 * hour, "1 d ago"],
    [13 * day, "13 d ago"],
    [29 * day, "29 d ago"],
  ])("%d ms ago is %s", (delta, text) => expect(rel(delta)).toBe(text));

  it("goes to the absolute date after thirty days, where 'ago' stops helping", () => {
    expect(rel(31 * day)).toBe("2026-09-16 UTC");
  });

  it("says 'in' for a time ahead of the clock, and 'just now' within the usual clock skew", () => {
    expect(formatRelative(T + 3 * sec, T)).toBe("just now");
    expect(formatRelative(T + 10 * min, T)).toBe("in 10 min");
    expect(formatRelative(T + 3 * hour, T)).toBe("in 3 h");
  });

  it("does not throw on nonsense", () => {
    expect(formatRelative(Number.NaN, T)).toBe("unknown time");
    expect(formatRelative(T, Number.NaN)).toBe("unknown time");
  });
});

describe("the injectable clock", () => {
  it("uses the clock it is given, so a test can freeze time", () => {
    const fmt = createTimeFormatter(() => T);
    expect(fmt.now()).toBe(T);
    expect(fmt.relative(T - 4 * min)).toBe("4 min ago");
  });

  it("defaults to the system clock", () => {
    const before = Date.now();
    const now = systemClock();
    expect(now).toBeGreaterThanOrEqual(before);
    expect(now).toBeLessThanOrEqual(Date.now());
  });
});
