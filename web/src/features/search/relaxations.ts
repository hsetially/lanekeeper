import { useEffect, useRef, useState } from "react";
import { searchText } from "@/api/generated/endpoints/search/search";
import { textKey, textParams } from "./text-request";
import type { SearchParams } from "./url";

// "No matches" is followed by the searches that WOULD match (the design's frame 1e: "suggestions are real searches, each with
// its match count"). Each suggestion relaxes ONE thing about the search that found nothing: switch to regex, ignore case, search
// every swimlane, clear the path glob. Each is asked for as a real search, with a small cap, and is shown only if it found
// something. At most three searches run, they are cancelled when the search changes, and they never run for a search that found
// something.

export type Relaxation = { id: string; label: string; patch: Partial<SearchParams>; params: SearchParams };

/** A pattern character: only a pattern could have meant it as more than itself. */
const PATTERN_CHARACTER = /[.*+?^${}()|[\]\\]/;

/** The relaxations that make sense for this search, in the order the design lists them. */
export function relaxations(params: SearchParams, laneCount: number): Relaxation[] {
  const make = (id: string, label: string, patch: Partial<SearchParams>): Relaxation => ({ id, label, patch, params: { ...params, ...patch } });
  const out: Relaxation[] = [];
  if (!params.regex && PATTERN_CHARACTER.test(params.q)) out.push(make("regex", "Switch to regex", { regex: true }));
  if (params.caseSensitive) out.push(make("case", "Ignore case", { caseSensitive: false }));
  if (params.lanes !== null && params.lanes.length > 0 && laneCount > params.lanes.length) out.push(make("lanes", `Search all ${laneCount} swimlanes`, { lanes: null }));
  if (params.glob !== "") out.push(make("glob", "Clear the path glob", { glob: "" }));
  return out;
}

export type RelaxedCount = { matches: number; files: number; capped: boolean };

/** How many matches a relaxed search finds; a search that found none, or failed, has no entry. */
export function useRelaxedCounts(candidates: readonly Relaxation[] | null): ReadonlyMap<string, RelaxedCount> {
  const [counts, setCounts] = useState<ReadonlyMap<string, RelaxedCount>>(new Map());
  // The key is the searches, so a new array of the same searches does not ask again.
  const key = candidates === null ? null : candidates.map((c) => textKey(c.params, "relaxed")).join("|");
  const latest = useRef(candidates);
  useEffect(() => {
    latest.current = candidates;
  });
  useEffect(() => {
    const list = latest.current;
    setCounts(new Map());
    if (key === null || list === null || list.length === 0) return undefined;
    const controller = new AbortController();
    for (const candidate of list.slice(0, 3)) {
      searchText(textParams(candidate.params, { max_total: 100 }), { signal: controller.signal }).then(
        (found) => {
          if (controller.signal.aborted || found.hits.length === 0) return;
          const files = new Set(found.hits.map((h) => `${h.swimlane}\u0000${h.path}`)).size;
          setCounts((previous) => new Map(previous).set(candidate.id, { matches: found.hits.length, files, capped: found.truncated }));
        },
        () => undefined,
      );
    }
    return () => {
      controller.abort();
    };
  }, [key]);
  return counts;
}
