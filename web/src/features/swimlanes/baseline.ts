import { BaselineState, type BaselineStatus } from "@/api/generated/model";
import { formatTime } from "@/lib/time";

// The baseline of a swimlane, and what it locks. `SwimlaneSummary.baseline` states it for every swimlane the registry knows;
// it is absent only in a response from a server that predates the field, and then the screens say nothing about it rather than
// claim the baseline is confirmed (or not).
//
// The contract has no cause on a 423 (CG4b is not in it), so the explanation of a locked restart is derived here from the
// operation and the baseline (design/DEVIATIONS.md): the baseline is the one lock the screens can name for certain.

export type BaselineView =
  | { readonly kind: "confirmed"; readonly by: string | null; readonly at: number | null }
  | { readonly kind: "unconfirmed" }
  | { readonly kind: "in_progress" }
  /** The server did not say. */
  | { readonly kind: "unknown" };

export function baselineView(baseline: BaselineStatus | null | undefined): BaselineView {
  if (baseline === null || baseline === undefined) return { kind: "unknown" };
  switch (baseline.state) {
    case BaselineState.confirmed:
      return { kind: "confirmed", by: baseline.by?.display_name ?? null, at: baseline.at };
    case BaselineState.unconfirmed:
      return { kind: "unconfirmed" };
    case BaselineState.in_progress:
      return { kind: "in_progress" };
    default:
      return { kind: "unknown" };
  }
}

export type EditLock = { readonly kind: "unconfirmed" | "in_progress"; readonly title: string; readonly text: string };

/** The "Editing locked" banner of the swimlane screens, or null when the baseline is confirmed or not stated. */
export function editLock(swimlane: string, baseline: BaselineStatus | null | undefined): EditLock | null {
  const view = baselineView(baseline);
  if (view.kind === "unconfirmed") return { kind: "unconfirmed", title: "Editing locked", text: `${swimlane} has no confirmed baseline. An admin must finish adoption first.` };
  if (view.kind === "in_progress") return { kind: "in_progress", title: "Editing locked", text: `${swimlane} has an adoption in progress. Editing stays locked until an admin confirms the baseline.` };
  return null;
}

/** What a 423 on a write to this swimlane most likely means, for a screen to say next to the server's own title. */
export function lockedExplanation(swimlane: string, baseline: BaselineStatus | null | undefined, until: number | null): { title: string; detail: string } {
  const view = baselineView(baseline);
  if (view.kind === "unconfirmed") {
    return { title: `${swimlane} is locked until it is adopted`, detail: "Its baseline is unconfirmed, so edits and restarts are blocked. An admin needs to adopt it first." };
  }
  if (view.kind === "in_progress") {
    return { title: `${swimlane} has an adoption in progress`, detail: "Edits and restarts are blocked until an admin confirms the baseline." };
  }
  const ends = until === null ? "" : ` Locked until ${formatTime(until)}.`;
  return { title: `${swimlane} is locked`, detail: `Another operation is using it. Try again in a moment.${ends}` };
}
