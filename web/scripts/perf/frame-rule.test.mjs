import { describe, expect, it } from "vitest";
import { controlVerdict, DROPPED_FRAME_MS, droppedAt, uncappedFps } from "./frame-rule.mjs";

/** Timestamps of `n` frames, `interval` ms apart, with `late` frames (index: extra ms) arriving late. */
const frames = (n, interval, late = {}) => {
  const out = [0];
  for (let i = 1; i < n; i++) out.push(/** @type {number} */ (out[i - 1]) + interval + (late[i] ?? 0));
  return out;
};

describe("uncappedFps", () => {
  it("is the number of frame intervals over their total time, and is not capped at 60", () => {
    expect(uncappedFps(frames(301, 1000 / 60))).toBeCloseTo(60, 6);
    expect(uncappedFps(frames(301, 5))).toBeCloseTo(200, 6);
    expect(uncappedFps(frames(301, 1000 / 240))).toBeCloseTo(240, 6);
  });

  it("falls below 60 when frames take longer than 16.7 ms, so a grid that cannot make 60 does not pass", () => {
    expect(uncappedFps(frames(301, 20))).toBeCloseTo(50, 6);
  });

  it("counts a slow frame in the mean, as the time it took", () => {
    // 299 frames of 5 ms and one of 105 ms: 300 intervals in 1,595 ms.
    expect(uncappedFps(frames(301, 5, { 150: 100 }))).toBeCloseTo((300 * 1000) / 1600, 6);
  });

  it("refuses a run that has no rate: fewer than two frames, or no time between them", () => {
    expect(() => uncappedFps([])).toThrow(/at least two frames/);
    expect(() => uncappedFps([10])).toThrow(/at least two frames/);
    expect(() => uncappedFps([10, 10])).toThrow(/span 0 ms/);
    expect(() => uncappedFps([10, Number.NaN])).toThrow(/span NaN ms/);
  });
});

describe("droppedAt", () => {
  it("lists the frames that arrived more than 25 ms after the one before, by index", () => {
    expect(droppedAt(frames(100, 16.7, { 4: 17, 60: 30 }))).toEqual([4, 60]);
  });

  it("does not count a frame at the threshold, nor an ordinary frame", () => {
    expect(droppedAt([0, DROPPED_FRAME_MS, 2 * DROPPED_FRAME_MS + 0.01])).toEqual([2]);
    expect(droppedAt(frames(50, 16.7))).toEqual([]);
  });

  it("answers nothing for no frames or one", () => {
    expect(droppedAt([])).toEqual([]);
    expect(droppedAt([5])).toEqual([]);
  });
});

describe("controlVerdict", () => {
  it("passes a grid that drops no more than the control list does", () => {
    expect(controlVerdict({ gridDropped: 2, controlDropped: 5, margin: 3 })).toMatchObject({ passed: true, allowed: 8 });
    expect(controlVerdict({ gridDropped: 0, controlDropped: 0, margin: 0 })).toMatchObject({ passed: true, allowed: 0 });
  });

  it("passes at exactly the control plus the margin, and fails one frame over", () => {
    expect(controlVerdict({ gridDropped: 8, controlDropped: 5, margin: 3 }).passed).toBe(true);
    expect(controlVerdict({ gridDropped: 9, controlDropped: 5, margin: 3 }).passed).toBe(false);
  });

  it("fails a grid that makes the compositor slower: hundreds of late frames against a handful", () => {
    for (const gridDropped of [40, 150, 1500]) {
      const verdict = controlVerdict({ gridDropped, controlDropped: 4, margin: 8 });
      expect(verdict.passed, `${gridDropped} dropped`).toBe(false);
      expect(verdict.message).toMatch(/making frames late/);
    }
  });

  it("says what it compared, whichever way it came out", () => {
    expect(controlVerdict({ gridDropped: 1, controlDropped: 2, margin: 3 }).message).toMatch(/dropped 1 frame\(s\), the control list 2, .* 5 /);
    expect(controlVerdict({ gridDropped: 30, controlDropped: 2, margin: 3 }).message).toMatch(/dropped 30 frame\(s\) where the control list .* dropped 2: more than the 5 it may/);
  });

  it.each([
    [{ gridDropped: -1, controlDropped: 0, margin: 0 }, /gridDropped/],
    [{ gridDropped: 1.5, controlDropped: 0, margin: 0 }, /gridDropped/],
    [{ gridDropped: 0, controlDropped: Number.NaN, margin: 0 }, /controlDropped/],
    [{ gridDropped: 0, controlDropped: 0, margin: -1 }, /margin/],
    [{ gridDropped: 0, controlDropped: 0, margin: Number.POSITIVE_INFINITY }, /margin/],
  ])("refuses counts that are not whole frames, instead of passing: %j", (counts, message) => {
    expect(() => controlVerdict(counts)).toThrow(message);
  });
});
