import { Severity, type ListFindingsParams } from "@/api/generated/model";

// Severity as the person reads it: four levels, most severe first. DISPLAY ONLY. A severity is the server's: the registry rates a
// change or a finding by the engine's defaults and the admin's rules (D76, D103), and the browser never rates or re-rates one. This
// module puts rows in an order, keeps the rows at or above a level, and says plainly when it can do that only for the rows that
// are loaded. Where the server can filter (`min_severity` on the findings list) the screen asks it to and shows the whole answer;
// everywhere else the filter and the sort work on the loaded rows and `loadedScopeNote` says so.

/** Most severe first. */
export const SEVERITIES: readonly Severity[] = [Severity.critical, Severity.high, Severity.medium, Severity.low];

export const SEVERITY_LABEL: Readonly<Record<Severity, string>> = {
  [Severity.critical]: "Critical",
  [Severity.high]: "High",
  [Severity.medium]: "Medium",
  [Severity.low]: "Low",
};

/** 0 for critical up to 3 for low. */
export const severityRank = (severity: Severity): number => SEVERITIES.indexOf(severity);

type Rated = Severity | null | undefined;

/** Negative when `a` is the more severe. A row nobody rated comes after every rated row. */
export function compareSeverity(a: Rated, b: Rated): number {
  const rank = (s: Rated): number => (s === null || s === undefined ? SEVERITIES.length : severityRank(s));
  return rank(a) - rank(b);
}

/** A new array, the most severe first and the unrated last. Rows that tie keep their order, or `then`'s. */
export function sortBySeverity<T>(items: readonly T[], severityOf: (item: T) => Rated, then?: (a: T, b: T) => number): T[] {
  return [...items].sort((a, b) => compareSeverity(severityOf(a), severityOf(b)) || (then?.(a, b) ?? 0));
}

/** True for `min` and every level above it. A row with no severity is at no level. */
export function atLeast(severity: Rated, min: Severity): boolean {
  return severity !== null && severity !== undefined && severityRank(severity) <= severityRank(min);
}

/** The rows at `min` and above, or the same array when no level is asked for. */
export function filterAtLeast<T>(items: readonly T[], severityOf: (item: T) => Rated, min: Rated): readonly T[] {
  return min === null || min === undefined ? items : items.filter((item) => atLeast(severityOf(item), min));
}

export type MinSeverityChoice = { readonly value: Severity | "any"; readonly label: string };

/** The Findings screen's menu: a minimum, not a pick of any, because that is what the server filters by. */
export const MIN_SEVERITY_CHOICES: readonly MinSeverityChoice[] = [
  { value: "any", label: "Any severity" },
  { value: Severity.medium, label: `${SEVERITY_LABEL.medium} and above` },
  { value: Severity.high, label: `${SEVERITY_LABEL.high} and above` },
  { value: Severity.critical, label: `${SEVERITY_LABEL.critical} only` },
];

export function minSeverityLabel(min: Rated): string {
  if (min === null || min === undefined) return "Any severity";
  return min === Severity.critical ? `${SEVERITY_LABEL.critical} only` : `${SEVERITY_LABEL[min]} and above`;
}

/** A level from text that came from an address or a form, or undefined for anything that is not exactly one. */
export function parseSeverity(value: unknown): Severity | undefined {
  return typeof value === "string" ? Object.values(Severity).find((s) => s === value) : undefined;
}

/** The part of a request to the findings list that asks the server for a minimum. */
export function minSeverityParam(min: Rated): Pick<ListFindingsParams, "min_severity"> {
  return min === null || min === undefined ? {} : { min_severity: min };
}

/**
 * What to tell the person when a filter or a sort reaches only the rows that are loaded: null when every row is (the answer is
 * whole), else a sentence that says so. Never leave a screen to look complete when it is not.
 */
export function loadedScopeNote({ what, count, hasMore, noun = "row" }: { what: "filter" | "sort"; count: number; hasMore: boolean; noun?: string }): string | null {
  if (!hasMore) return null;
  const rows = `${count} ${count === 1 ? noun : `${noun}s`} loaded so far`;
  return what === "filter" ? `Applies to the ${rows}. More are not loaded.` : `Sorted within the ${rows}. More are not loaded.`;
}
