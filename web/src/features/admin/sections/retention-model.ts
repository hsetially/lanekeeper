import type { RetentionInput, RetentionPreview } from "@/api/generated/model";

// The retention policy as numbers and words. WHAT A POLICY WOULD DELETE IS THE HUB'S ANSWER (the dry run, `dry_run=true`): nothing
// here estimates it. The bounds are the contract's (1 to 3,650 days, 1 to 10,000 versions) with the design's tighter floors (30 days,
// 5 versions), kept in this page so that a typing slip cannot keep next to nothing (design/DEVIATIONS.md).

export const MIN_DAYS = 30;
export const MAX_DAYS = 3650;
export const MIN_LAST = 5;
export const MAX_LAST = 10_000;

/** Typed to save a policy that deletes: words, so that it is typed on purpose and is not a number to copy from the screen. */
export const CONFIRM_PHRASE = "delete versions";

export type RetentionErrors = { days?: string; last?: string };
export type ParsedRetention = { ok: true; value: RetentionInput } | { ok: false; errors: RetentionErrors };

const WHOLE = /^\d+$/;

/** Both fields at once. A message never repeats the value that was typed. */
export function parseRetention(daysText: string, lastText: string): ParsedRetention {
  const errors: RetentionErrors = {};
  const days = daysText.trim();
  const last = lastText.trim();
  let keepDays = 0;
  let keepLast = 0;
  if (days === "") errors.days = "Enter the number of days";
  else if (!WHOLE.test(days)) errors.days = "Enter a whole number of days";
  else {
    keepDays = Number(days);
    if (keepDays < MIN_DAYS) errors.days = `Minimum is ${MIN_DAYS} days`;
    else if (keepDays > MAX_DAYS) errors.days = `Maximum is ${MAX_DAYS.toLocaleString("en-US")} days`;
  }
  if (last === "") errors.last = "Enter the number of versions";
  else if (!WHOLE.test(last)) errors.last = "Enter a whole number";
  else {
    keepLast = Number(last);
    if (keepLast < MIN_LAST) errors.last = `Keep at least ${MIN_LAST} versions per file`;
    else if (keepLast > MAX_LAST) errors.last = `Maximum is ${MAX_LAST.toLocaleString("en-US")} versions`;
  }
  if (errors.days !== undefined || errors.last !== undefined) return { ok: false, errors };
  return { ok: true, value: { keep_days: keepDays, keep_last_versions: keepLast } };
}

export const sameValues = (a: RetentionInput, b: RetentionInput): boolean => a.keep_days === b.keep_days && a.keep_last_versions === b.keep_last_versions;

/** "every version for 180 days and the last 50 versions of each file". */
export const policySentence = (policy: RetentionInput): string => `every version for ${policy.keep_days} days and the last ${policy.keep_last_versions} versions of each file`;

export const formatCount = (n: number): string => n.toLocaleString("en-US");

const UNITS = ["KiB", "MiB", "GiB", "TiB"] as const;

/** Binary units, one decimal: 3.2 GiB. The job's limits are in MiB, so the units are the same. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "unknown";
  if (bytes < 1024) return `${formatCount(Math.round(bytes))} B`;
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${UNITS[unit]}`;
}

/** The dry run found something to delete. */
export const wouldDelete = (preview: RetentionPreview): boolean => preview.versions_to_delete > 0 || preview.bytes_to_free > 0;
