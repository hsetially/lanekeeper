import type { LucideIcon } from "lucide-react";
import type { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { Columns2, History, Layers, Search, Settings, ShieldAlert, CircleCheck, Zap } from "@/icons";
import type { RouteId } from "@/routes/props";
import { routeById } from "@/routes/table";

// The sidebar's items (the Shell frames): three groups, eight items. WHO SEES an item is not decided here: an item is shown
// when the role may open its route (src/routes/table.ts), so the sidebar offers exactly what opens and never a link to a screen
// that would answer with the permission state. The route's role is the handoff's, except where src/routes/overrides.ts records
// a decision (Audit and Approvals are Admin-only). "Viewer: hide destinations; disable actions with a reason" is the handoff's
// rule for navigation (Shell frame 1l); the alternative with locks (frame 1m) is not built.

export type NavChild = { readonly label: string; readonly search: { readonly type: "text" | "settings" | "docs" }; /** The key hint beside it. */ readonly hint?: string };

export type NavEntry = {
  readonly id: string;
  readonly label: string;
  readonly icon: LucideIcon;
  readonly route: RouteId;
  /** A count beside the label, from live data (nav-badges.ts). */
  readonly badge?: "findings" | "approvals" | "admin";
  readonly children?: readonly NavChild[];
};

export const NAV_GROUPS: readonly (readonly NavEntry[])[] = [
  [
    { id: "swimlanes", label: "Swimlanes", icon: Layers, route: "swimlanes" },
    { id: "compare", label: "Compare", icon: Columns2, route: "compare" },
    {
      id: "search",
      label: "Search",
      icon: Search,
      route: "search",
      children: [{ label: "Text", search: { type: "text" }, hint: "/" }, { label: "Settings", search: { type: "settings" } }, { label: "Docs", search: { type: "docs" } }],
    },
  ],
  [
    { id: "status", label: "Feature status", icon: Zap, route: "status" },
    { id: "findings", label: "Findings", icon: ShieldAlert, route: "findings", badge: "findings" },
    { id: "approvals", label: "Approvals", icon: CircleCheck, route: "approvals", badge: "approvals" },
  ],
  [
    { id: "audit", label: "Audit", icon: History, route: "audit" },
    { id: "admin", label: "Admin", icon: Settings, route: "admin-users", badge: "admin" },
  ],
];

/** Whether `role` may open the route of an entry. A person with no role may open nothing but the public screens, which have no sidebar. */
export function mayOpen(role: Role | null, route: RouteId): boolean {
  const need = routeById(route).access;
  return need === "public" || roleAtLeast(role, need);
}

/** The groups a role sees: an item it may not open is dropped, and a group with nothing left is dropped. */
export function visibleGroups(role: Role | null): NavEntry[][] {
  return NAV_GROUPS.map((group) => group.filter((entry) => mayOpen(role, entry.route))).filter((group) => group.length > 0);
}

/**
 * `99+` above 99: a count is a reason to look, not a figure to read. `more` says the count is a floor, because the hub has more
 * than the page it sent (`25+`).
 */
export const formatBadge = (count: number, more = false): string => (count > 99 || (more && count >= 99) ? "99+" : `${count}${more ? "+" : ""}`);

// Which item a screen belongs under, so that the item is marked current (`aria-current="page"`) on the screen's own routes and
// on the ones below it: a file is under Swimlanes, a doc under Search, an approval under Approvals. The account screens
// (My GitHub token, personal tokens) and the error screens belong to no item.
const ITEM_OF_ROUTE: Readonly<Record<string, string>> = {
  swimlanes: "swimlanes",
  swimlane: "swimlanes",
  files: "swimlanes",
  services: "swimlanes",
  "swimlane-findings": "swimlanes",
  "swimlane-audit": "swimlanes",
  adopt: "swimlanes",
  drift: "swimlanes",
  activity: "swimlanes",
  draft: "swimlanes",
  compare: "compare",
  search: "search",
  doc: "search",
  "doc-view": "search",
  status: "status",
  findings: "findings",
  approvals: "approvals",
  approval: "approvals",
  "proposal-link": "approvals",
  audit: "audit",
  "admin-users": "admin",
  "admin-swimlanes": "admin",
  "admin-section": "admin",
};

export function navIdForRoute(routeId: string | undefined): string | null {
  return routeId === undefined ? null : (ITEM_OF_ROUTE[routeId] ?? null);
}
