// How long to wait before the live stream is opened again (rule 5: every retry is bounded and jittered).
//
// The ceiling climbs 1, 2, 4, 8 and then 15 seconds and stays there. The delay is drawn from the WHOLE range below the
// ceiling ("full jitter"), so a hub restart that drops every tab at once does not bring every tab back at once. Two bounds
// make that safe: a minimum (a run of low draws must not become a busy loop against a server that is struggling) and a
// floor the server may set (the `retry:` hint after it closed the stream cleanly, `Retry-After` on a 429).

export const BACKOFF_BASE_MS = 1000;
export const BACKOFF_CAP_MS = 15_000;
/** The shortest wait, whatever the draw. */
export const BACKOFF_MIN_MS = 200;
/** A server's floor is honoured up to this: a hub that says "wait an hour" is retried after a minute. */
export const MAX_FLOOR_MS = 60_000;

/** The top of the range for the nth consecutive failed attempt (0 is the first). */
export function backoffCeilingMs(attempt: number): number {
  const n = Number.isFinite(attempt) ? Math.max(0, Math.min(30, Math.floor(attempt))) : 30;
  return Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** n);
}

export type BackoffOptions = {
  /** The least the server wants us to wait, in milliseconds. */
  floorMs?: number;
  /** A draw in [0, 1). Tests pass their own. */
  random?: () => number;
};

/** A whole number of milliseconds between the floor (or the minimum) and the ceiling, uniformly. */
export function backoffDelayMs(attempt: number, { floorMs = 0, random = Math.random }: BackoffOptions = {}): number {
  const floor = Number.isFinite(floorMs) && floorMs > 0 ? Math.min(floorMs, MAX_FLOOR_MS) : 0;
  const low = Math.max(BACKOFF_MIN_MS, floor);
  // With a floor the range stays at least as wide as the floor again, so clients held back by the same hint still spread.
  const high = Math.max(low, backoffCeilingMs(attempt), floor > 0 ? 2 * floor : 0);
  const draw = Math.min(Math.max(random(), 0), 0.999_999_999);
  return low + Math.floor(draw * (high - low + 1));
}
