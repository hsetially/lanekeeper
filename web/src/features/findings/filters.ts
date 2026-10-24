import { Severity, type FindingKind, type ListFindingsParams } from "@/api/generated/model";
import { linkTo } from "@/routes/links";
import type { SearchOf } from "@/routes/props";
import { KINDS, codeOf, kindOfCode } from "./kinds";

// The filters of the Findings screen: which swimlanes, which checks, and how severe at least. They live in the ADDRESS
// (`lanes`, `code` or `kind`, `severity`), so a link keeps them and another screen can link to "the C8 findings of sitb". What
// the address says is untrusted input: a value that is not one of the contract's is dropped, so a typo shows everything and not
// nothing. The server does the filtering it can (one swimlane, the checks, the severity "at least"); a choice of SEVERAL swimlanes
// is narrowed here, in the findings that are loaded, because the server's `swimlane` takes one.

export type Filters = {
  readonly lanes: readonly string[];
  readonly kinds: readonly FindingKind[];
  /** Findings of this severity and above. */
  readonly minSeverity: Severity | null;
};

export const NO_FILTERS: Filters = { lanes: [], kinds: [], minSeverity: null };

/** The most swimlanes a filter holds. The design's scale is 40; this is the address's own bound. */
export const MAX_LANES = 64;

// SwimlaneId in api/openapi.yaml.
const LANE_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;

const words = (value: string | undefined): string[] => (value === undefined ? [] : value.split(",").filter((word) => word !== ""));

type Address = SearchOf<"findings">;

export function filtersFromSearch(search: Address): Filters {
  const named = new Set<FindingKind>();
  for (const code of words(search.code)) {
    const kind = kindOfCode(code);
    if (kind !== undefined) named.add(kind);
  }
  for (const name of words(search.kind)) {
    const kind = KINDS.find((k) => k === name);
    if (kind !== undefined) named.add(kind);
  }
  const lanes: string[] = [];
  // `swimlane` is what the links of the MCP tools say (`/findings?swimlane=sitb`); the screen's own `lanes` wins.
  for (const id of words(search.lanes ?? search.swimlane)) {
    if (LANE_ID.test(id) && !lanes.includes(id) && lanes.length < MAX_LANES) lanes.push(id);
  }
  const minSeverity = Object.values(Severity).find((s) => s === search.severity) ?? null;
  return { lanes, kinds: KINDS.filter((kind) => named.has(kind)), minSeverity };
}

export const isFiltered = (filters: Filters): boolean => filters.lanes.length > 0 || filters.kinds.length > 0 || filters.minSeverity !== null;

/** What the address holds for these filters. Nothing for a filter that is not set. */
export function searchOf(filters: Filters): Address {
  return {
    ...(filters.kinds.length === 0 ? {} : { code: filters.kinds.map(codeOf).join(",") }),
    ...(filters.lanes.length === 0 ? {} : { lanes: filters.lanes.join(",") }),
    ...(filters.minSeverity === null ? {} : { severity: filters.minSeverity }),
  };
}

export const hrefOf = (filters: Filters): string => linkTo("findings", undefined, searchOf(filters));

/** What the server is asked: one swimlane, the checks and the severity "at least". Never a cursor or a page size. */
export function serverParams(filters: Filters): Omit<ListFindingsParams, "cursor" | "limit"> {
  const [only, ...more] = filters.lanes;
  return {
    ...(only !== undefined && more.length === 0 ? { swimlane: only } : {}),
    ...(filters.kinds.length === 0 ? {} : { kinds: [...filters.kinds] }),
    ...(filters.minSeverity === null ? {} : { min_severity: filters.minSeverity }),
  };
}

/** The swimlanes to keep when the person chose several (the server filters by one), or null when there is nothing to narrow here. */
export function clientLanes(filters: Filters): ReadonlySet<string> | null {
  return filters.lanes.length > 1 ? new Set(filters.lanes) : null;
}

/** The list with `item` added, or removed if it is there. A new array. */
export function toggled<T>(list: readonly T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}
