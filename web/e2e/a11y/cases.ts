// Every screen and every state that the accessibility spec (and, for the screens, the layout spec) looks at. One list per area;
// an entry is a scenario, an address and (for an open dialog, a menu, a drawer, a held or failed answer) the few steps that
// bring the page into the state. The ids are unique; the spec fails on a duplicate.
import { adminCases } from "./cases-admin.ts";
import { compareSearchCases } from "./cases-compare-search.ts";
import { fileCases } from "./cases-file.ts";
import { findingsApprovalsCases } from "./cases-findings-approvals.ts";
import type { Case } from "./helpers.ts";
import { shellAccessCases } from "./cases-shell-access.ts";
import { swimlanesCases } from "./cases-swimlanes.ts";

export const ALL_CASES: readonly Case[] = [...shellAccessCases, ...swimlanesCases, ...fileCases, ...compareSearchCases, ...findingsApprovalsCases, ...adminCases];

const seen = new Set<string>();
for (const c of ALL_CASES) {
  if (seen.has(c.id)) throw new Error(`duplicate case id: ${c.id}`);
  seen.add(c.id);
}
