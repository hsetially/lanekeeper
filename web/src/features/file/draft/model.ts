import type { ProposalStatus } from "@/api/generated/model";

// The words of the draft page: what each status of a draft means for the person who made it. (The badge of a pickup state is
// `PickupChip`'s, from components/pickup: the one mapping is features/swimlanes/pickup.ts.)

export type Closed = { tone: "info" | "success" | "warning"; title: string; text: string };

/**
 * What a draft that is not waiting to be applied says. `draft` is the one status that can be applied, so it says nothing here.
 * A status never comes from the clock: the server says it is expired, and the page believes it.
 */
export function closedBy(status: ProposalStatus): Closed | null {
  switch (status) {
    case "draft":
      return null;
    case "pending":
      return { tone: "info", title: "Sent for approval", text: "An Admin needs to approve this change before it is applied. Nothing has changed on NFS yet." };
    case "approved":
      return { tone: "info", title: "Approved", text: "An Admin approved this change. It cannot be applied again from here." };
    case "rejected":
      return { tone: "warning", title: "Rejected", text: "An Admin rejected this change. Nothing was applied." };
    case "stale":
      return { tone: "warning", title: "The file changed after this draft was made", text: "This draft can no longer be applied. Open the file and make the change again." };
    case "expired":
      return { tone: "warning", title: "This draft expired", text: "Drafts are kept for a short time. Open the file and make the change again." };
    case "applied":
      return { tone: "success", title: "This draft was applied", text: "The change is on NFS." };
  }
}

/** Why Apply is off for a draft that cannot be applied, in a few words (the tooltip of the disabled button). */
export function cannotApply(status: ProposalStatus): string | null {
  switch (status) {
    case "draft":
      return null;
    case "applied":
      return "This draft was already applied.";
    case "pending":
      return "This draft is waiting for approval.";
    default:
      return "This draft can no longer be applied.";
  }
}

/** How many services the draft page lists. The contract allows 1,000. */
export const MAX_EFFECTS = 50;
