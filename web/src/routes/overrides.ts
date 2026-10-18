// Every place where the role that opens a screen is NOT what the handoff says, with the reason. The route table
// (src/routes/table.ts) holds the roles that apply; this file is the reviewed list of where and why they differ from the
// handoff's, so that a person reading a role in the table can find the decision behind it. `table.test.ts` checks the table
// against the handoff with exactly these differences allowed, and the shell's role-matrix test (features/shell/nav.test.ts)
// checks that the sidebar shows what the table lets each role open and nothing else.

import { Role } from "@/api/generated/model";
import type { RouteId } from "./props";

export type RoleDifference = {
  /** What a person sees: the sidebar item or the tab. */
  item: string;
  route: RouteId;
  /** What the handoff says. */
  handoff: string;
  /** What the contract (api/openapi.yaml) allows. */
  contract: string;
  /** What the app does. */
  chosen: Role;
  /** The plan's decision, or the open question that holds it. */
  reason: string;
};

export const ROLE_DIFFERENCES: readonly RoleDifference[] = [
  {
    item: "Audit",
    route: "audit",
    handoff: "Viewer and above (the Shell shows it to every role)",
    contract: "Admin only (`adminListAudit`)",
    chosen: Role.admin,
    reason: "W11: the contract's audit reads are Admin-only, and the design has no Viewer-safe audit.",
  },
  {
    item: "Swimlane audit (a tab of the swimlane)",
    route: "swimlane-audit",
    handoff: "Viewer and above",
    contract: "Admin only (`adminListAudit`)",
    chosen: Role.admin,
    reason: "W11, as the Audit item.",
  },
  {
    item: "Activity (a tab of the swimlane)",
    route: "activity",
    handoff: "Viewer and above",
    contract: "Admin only, through `adminListAudit`, until a viewer-scoped feed exists (CG9)",
    chosen: Role.admin,
    reason: "W11.",
  },
  {
    item: "Approvals",
    route: "approvals",
    handoff: "Editor and above to open, Admin to decide (the route table); Admin only in the Shell and the States index",
    contract: "A Viewer may call `listProposals`",
    chosen: Role.admin,
    reason: "W12: the prompt says approvals (admin), and the Shell and States P05 agree.",
  },
  {
    item: "Approval (the drawer)",
    route: "approval",
    handoff: "Editor and above",
    contract: "A Viewer may call `getProposal`",
    chosen: Role.admin,
    reason: "W12, as Approvals.",
  },
];
