// The two numbers of the 5,000-row grid gate (P10.grid_5000_rows_fps, W20 as the human decided it on 2026-10-17: "capacity plus
// control"), as pure functions the spec (e2e/perf/grid-5000.spec.ts) calls and a unit test pins.
//
//  1. CAPACITY: how many frames a second the grid's own work lets the browser make. The scroll moves the same distance in every
//     frame (a step of 1/N of the extent), in a Chromium whose frame-rate limit is off, so a frame takes as long as its work takes
//     and the mean frame interval is the cost of a frame. It is NOT capped at 60: the budget is a floor of 60, and a grid that
//     can make 180 frames a second has room, where one that can make 50 does not. This is the number written as `values.min`.
//
//  2. CONTROL: the frames a person would see dropped, judged against a hand-written list on the same machine in the same run.
//     A machine that drops a frame at the start of every scroll (a compositor stall that no page code causes) drops it for the
//     control list too, so the grid is only asked not to drop MORE than the control, by a margin written down and justified in
//     the spec. This is what keeps the gate from being blind to a grid that makes the compositor slower.

/** A frame that took longer than this to follow the one before it is a dropped frame (W20). */
export const DROPPED_FRAME_MS = 25;

/** @param {number[]} stamps requestAnimationFrame timestamps, in order */
function intervals(stamps) {
  const out = [];
  for (let i = 1; i < stamps.length; i++) out.push(/** @type {number} */ (stamps[i]) - /** @type {number} */ (stamps[i - 1]));
  return out;
}

/**
 * Frames a second the page made over a run: the number of intervals over their total time. Not capped.
 * @param {number[]} stamps
 * @returns {number}
 */
export function uncappedFps(stamps) {
  if (stamps.length < 2) throw new Error("a run needs at least two frames to have a rate");
  const total = /** @type {number} */ (stamps[stamps.length - 1]) - /** @type {number} */ (stamps[0]);
  if (!(total > 0) || !Number.isFinite(total)) throw new Error(`the frames of a run span ${String(total)} ms`);
  return ((stamps.length - 1) * 1000) / total;
}

/**
 * The frames of a run that were late, by the index of the frame that arrived late.
 * @param {number[]} stamps
 * @param {number} [thresholdMs]
 * @returns {number[]}
 */
export function droppedAt(stamps, thresholdMs = DROPPED_FRAME_MS) {
  return intervals(stamps).flatMap((interval, i) => (interval > thresholdMs ? [i + 1] : []));
}

/**
 * The control rule: over the same number of runs, in the same execution, the grid dropped no more frames than the control list
 * plus `margin`. Fails closed: a count that is not a whole number of frames, or a negative margin, is an error and not a pass.
 * @param {{ gridDropped: number; controlDropped: number; margin: number }} counts
 * @returns {{ passed: boolean; allowed: number; message: string }}
 */
export function controlVerdict({ gridDropped, controlDropped, margin }) {
  for (const [name, value] of Object.entries({ gridDropped, controlDropped, margin })) {
    if (!Number.isInteger(value) || value < 0) throw new Error(`${name} is a whole number of frames, 0 or more; got ${String(value)}`);
  }
  const allowed = controlDropped + margin;
  const passed = gridDropped <= allowed;
  return {
    passed,
    allowed,
    message: passed
      ? `the grid dropped ${gridDropped} frame(s), the control list ${controlDropped}, and the grid may drop ${allowed} (the control plus a margin of ${margin})`
      : `the grid dropped ${gridDropped} frame(s) where the control list on the same machine in the same run dropped ${controlDropped}: more than the ${allowed} it may (the control plus a margin of ${margin}). Something the grid does is making frames late`,
  };
}
