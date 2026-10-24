import { useCallback, useMemo } from "react";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { buildPrIndex, prBadgeFor, type PrBadgeInfo, type PrFacts } from "./badges";
import { prListQuery } from "./queries";

// The pull-request badges of one swimlane for the screens that list its files: the file header, the Drift tab and the findings.
// ONE hook call for the whole screen, never one per row: it reads the swimlane's links (two lists, one cache entry each, shared by
// every screen that asks for the same swimlane) and answers `badgeFor(file)` from memory.
//
//  - OPEN links are read to the end (a badge for every file with an open pull request), up to OPEN_PAGES pages of 100;
//  - MERGED links are read as the newest page only: a merge that is "awaiting sync" is a recent one, and the lists are newest first.
//
// The links change when a pull request does: `pr_state_changed` names the `['prs']` lists (src/live/invalidation.ts) and the
// lists on screen are read again. The events carry no swimlane (CG1), so every swimlane's lists are refetched, not only this one's.

const PAGE = 100;
const OPEN_PAGES = 10;
const MERGED_PAGES = 1;

export type PrBadges = {
  /** Both lists have been read. A screen says "No PR" only when this is true: while the lists load or when they failed it says nothing. */
  readonly ready: boolean;
  /** More open links exist than were read, so a file's badge may be missing. */
  readonly truncated: boolean;
  /** The badge of a file, or null. Null for every file until the lists have been read: a badge never flashes in and out. */
  readonly badgeFor: (facts: PrFacts) => PrBadgeInfo | null;
};

export function usePrBadges(swimlane: string, options: { enabled?: boolean } = {}): PrBadges {
  const enabled = options.enabled ?? true;
  const open = useCursorPages({ ...prListQuery({ swimlane, state: "open", limit: PAGE }), autoLoad: true, maxPages: OPEN_PAGES, enabled });
  const merged = useCursorPages({ ...prListQuery({ swimlane, state: "merged", limit: PAGE }), maxPages: MERGED_PAGES, enabled });

  const openItems = open.items;
  const mergedItems = merged.items;
  const index = useMemo(() => buildPrIndex(swimlane, [...openItems, ...mergedItems]), [swimlane, openItems, mergedItems]);
  const ready = enabled && open.status === "success" && merged.status === "success";
  const badgeFor = useCallback((facts: PrFacts): PrBadgeInfo | null => (ready ? prBadgeFor(index, facts) : null), [ready, index]);
  return { ready, truncated: open.truncated, badgeFor };
}
