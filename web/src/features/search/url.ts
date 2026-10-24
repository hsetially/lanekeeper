import type { SearchOf } from "@/routes/props";

// The state of the Search screen as it is written in the address (`/search?type=text&q=...`). The router keeps strings, so what
// the screen needs (a tab, two toggles, a list of swimlanes) is worked out here, once, and written back with `toSearch`. The
// address is untrusted: a swimlane that is not a swimlane id, a tab that is not a tab and a toggle that is not 1 are ignored.

export type SearchTab = "text" | "settings" | "docs";
export const TABS: readonly SearchTab[] = ["text", "settings", "docs"];

export type SearchParams = {
  tab: SearchTab;
  q: string;
  caseSensitive: boolean;
  regex: boolean;
  /** `null`: all swimlanes. `[]`: none (the person cleared the list). */
  lanes: string[] | null;
  glob: string;
};

/** The most swimlanes a search names (`swimlanes` has `maxItems: 64`). */
export const MAX_LANES = 64;
/** The address spells "no swimlane at all" `-`, which cannot be a swimlane id (an id starts with a letter or a digit). */
export const NO_LANES = "-";
const SWIMLANE_ID = /^[a-z0-9][a-z0-9-]{0,62}$/;

const isTab = (value: string | undefined): value is SearchTab => value !== undefined && (TABS as readonly string[]).includes(value);

function readLanes(value: string | undefined): string[] | null {
  if (value === undefined || value === "") return null;
  if (value === NO_LANES) return [];
  const seen = new Set<string>();
  for (const part of value.split(",")) {
    const id = part.trim();
    if (SWIMLANE_ID.test(id)) seen.add(id);
    if (seen.size === MAX_LANES) break;
  }
  return seen.size === 0 ? null : [...seen];
}

export function readSearch(search: SearchOf<"search">): SearchParams {
  // `mode` is what the links of the MCP tools use (`/search?mode=text&q=`); `type` is the screen's own, and wins.
  const tab = isTab(search.type) ? search.type : isTab(search.mode) ? search.mode : "text";
  return {
    tab,
    q: search.q ?? "",
    caseSensitive: search.cs === "1",
    regex: search.rx === "1",
    lanes: readLanes(search.lanes),
    glob: search.glob ?? "",
  };
}

/** The keys of the address for these parameters: the tab always, the rest only when it differs from the default. */
export function toSearch(params: SearchParams): NonNullable<SearchOf<"search">> {
  const out: Record<string, string> = { type: params.tab };
  if (params.q !== "") out["q"] = params.q;
  if (params.caseSensitive) out["cs"] = "1";
  if (params.regex) out["rx"] = "1";
  if (params.lanes !== null) out["lanes"] = params.lanes.length === 0 ? NO_LANES : params.lanes.join(",");
  if (params.glob !== "") out["glob"] = params.glob;
  return out;
}
