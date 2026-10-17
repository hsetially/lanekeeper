// Times (W18): the API's timestamps are milliseconds since the epoch. Absolute times are shown in UTC with the zone
// spelled out, because the logs are UTC and a screenshot in a ticket must mean the same thing for everyone, and because
// it makes the tests deterministic. Relative times ("4 min ago") are computed against a clock that a test can freeze.

export type Clock = () => number;
export const systemClock: Clock = () => Date.now();

const UNKNOWN = "unknown time";
// Dates the engine can show: 1970 to 9999. Anything else is a bad value, not a time.
const MAX_MS = 253_402_300_799_999;

const pad = (n: number, width = 2): string => String(n).padStart(width, "0");

function parts(ms: number): { date: string; hm: string; hms: string } | undefined {
  if (!Number.isFinite(ms) || ms < 0 || ms > MAX_MS) return undefined;
  const d = new Date(ms);
  const date = `${pad(d.getUTCFullYear(), 4)}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
  const hm = `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
  return { date, hm, hms: `${hm}:${pad(d.getUTCSeconds())}` };
}

/** `2026-10-17 06:42 UTC`, or with seconds `2026-10-17 06:42:05 UTC`. */
export function formatTime(ms: number, options: { seconds?: boolean } = {}): string {
  const p = parts(ms);
  if (!p) return UNKNOWN;
  return `${p.date} ${options.seconds === true ? p.hms : p.hm} UTC`;
}

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
// Within this much, a time in the future is the two clocks disagreeing, not a future event.
const SKEW = 5 * SECOND;

function span(ms: number): string {
  if (ms < 90 * SECOND) return "1 min";
  if (ms < HOUR) return `${Math.round(ms / MINUTE)} min`;
  if (ms < DAY) return `${Math.floor(ms / HOUR)} h`;
  return `${Math.floor(ms / DAY)} d`;
}

/** `4 min ago`, `in 3 h`, `just now`; after thirty days the date. */
export function formatRelative(ms: number, now: number): string {
  if (!Number.isFinite(ms) || !Number.isFinite(now)) return UNKNOWN;
  const delta = now - ms;
  if (delta < 0) {
    if (-delta <= SKEW) return "just now";
    return `in ${span(-delta)}`;
  }
  if (delta < 45 * SECOND) return "just now";
  if (delta >= 30 * DAY) {
    const p = parts(ms);
    return p ? `${p.date} UTC` : UNKNOWN;
  }
  // Minutes round, hours and days floor, so "59 min ago" never becomes "60 min ago".
  if (delta < HOUR) return `${delta < 90 * SECOND ? 1 : Math.min(59, Math.round(delta / MINUTE))} min ago`;
  return `${span(delta)} ago`;
}

export function createTimeFormatter(clock: Clock = systemClock) {
  return {
    now: clock,
    time: formatTime,
    relative: (ms: number): string => formatRelative(ms, clock()),
  };
}
