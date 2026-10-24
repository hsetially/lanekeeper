import type { ListPrsParams } from "@/api/generated/model";

// The state filter of the Pull requests page: the `state` search key. It is the three states of a link (`PrState`) and "all".

export const FILTERS = ["open", "merged", "closed", "all"] as const;
export type PrFilter = (typeof FILTERS)[number];

/** Open is the default: it is the list of pull requests that are waiting for something, and where a new one appears. */
export function filterOf(value: string | undefined): PrFilter {
  return FILTERS.find((filter) => filter === value) ?? "open";
}

const PAGE = 100;

/** The question for the list: one swimlane's links, in a state or in any, newest first (the server's order). */
export function listParams(swimlane: string, filter: PrFilter): Omit<ListPrsParams, "cursor"> {
  return filter === "all" ? { swimlane, limit: PAGE } : { swimlane, state: filter, limit: PAGE };
}
