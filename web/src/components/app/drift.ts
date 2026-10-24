import { DriftState } from "@/api/generated/model";
import type { StatusVariant } from "./StatusBadge";

// Drift states as badges. The mapping from the API's enum to a StatusBadge variant is written once, here, so that every
// screen shows a state with the same colour, icon and words.

export const DRIFT_VARIANT: Record<DriftState, StatusVariant> = {
  in_sync: "sync",
  git_ahead: "git",
  nfs_ahead: "nfs",
  conflict: "conflict",
  unknown: "unknown",
  untracked: "untracked",
  intentional_divergence: "intentional",
};

// The states that ask a person to look. In sync is the absence of them, and an intentional divergence is one a person decided
// to keep. Most urgent first: this is the order that names the colour of a swimlane's one badge.
const NEEDS_ATTENTION: readonly DriftState[] = [DriftState.conflict, DriftState.nfs_ahead, DriftState.git_ahead, DriftState.unknown, DriftState.untracked];

export type DriftSummary = { total: number; variant: StatusVariant; /** "drift", "unknown" or "In sync": what follows the count. */ word: string };

/**
 * A swimlane's drift counts as one badge: how many files need attention, the colour of the most urgent state, and "drift". A
 * swimlane whose only trouble is files of unknown state (not adopted yet) says "unknown", as the design's locked swimlane does.
 */
export function driftSummary(counts: readonly (readonly [DriftState, number])[]): DriftSummary {
  const by = new Map<DriftState, number>();
  for (const [state, count] of counts) {
    if (Number.isInteger(count) && count > 0) by.set(state, (by.get(state) ?? 0) + count);
  }
  const present = NEEDS_ATTENTION.filter((state) => by.has(state));
  const total = present.reduce((sum, state) => sum + (by.get(state) ?? 0), 0);
  const first = present[0];
  if (first === undefined) return { total: 0, variant: "sync", word: "In sync" };
  return { total, variant: DRIFT_VARIANT[first], word: present.length === 1 && first === DriftState.unknown ? "unknown" : "drift" };
}
