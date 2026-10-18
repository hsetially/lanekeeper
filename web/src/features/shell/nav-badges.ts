import { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { usePendingRequests } from "@/features/admin/users/pending";
import { proposalListQuery } from "@/features/approvals/queries";
import { swimlaneListQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";

// The two counts of the sidebar. Neither is its own request for the sake of a number when the data is already there:
//  - Findings is the sum of `finding_count` over the swimlane list, which the switcher and the swimlane screens read too, so
//    it is one cache entry and live events keep it fresh (findings_changed, swimlane_changed);
//  - Approvals is the pending proposals, Admin only (the screen is Admin-only), and a count is capped at 99+ by the caller;
//  - Admin is the access requests that wait for an Admin: the same cache entry as the Users section lists, kept fresh by
//    `access_request_created`, asked for only by an Admin.
// Both stay `null` until they are known (no number is better than a wrong one), and a failed request leaves no number.

export type NavBadges = {
  findings: number | null;
  approvals: number | null;
  /** The hub has more pending proposals than the page it sent (it sends at most 25): `approvals` is a floor, shown with a plus. */
  approvalsMore: boolean;
  /** The access requests that wait for an Admin. */
  admin: number | null;
  /** The hub has more waiting requests than the first page: `admin` is a floor, shown with a plus. */
  adminMore: boolean;
};

export function useNavBadges(role: Role | null): NavBadges {
  const signedIn = role !== null;
  const swimlanes = useCursorPages({ ...swimlaneListQuery(), autoLoad: true, enabled: signedIn });
  const isAdmin = roleAtLeast(role, Role.admin);
  const pending = useCursorPages({ ...proposalListQuery({ status: "pending", limit: 100 }), enabled: isAdmin });

  const findings = swimlanes.status === "success" && !swimlanes.hasMore ? swimlanes.items.reduce((sum, s) => sum + s.finding_count, 0) : null;
  // The hub sends at most 25 proposals a page whatever limit is asked, so "more pages" is the sign that the count is a floor.
  const approvals = isAdmin && pending.status === "success" ? pending.items.length : null;
  const requests = usePendingRequests({ enabled: isAdmin, autoLoad: false }).pages;
  const admin = isAdmin && requests.status === "success" ? requests.items.length : null;
  return { findings, approvals, approvalsMore: approvals !== null && pending.hasMore, admin, adminMore: admin !== null && requests.hasMore };
}
