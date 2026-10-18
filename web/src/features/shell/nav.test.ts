import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { Role } from "@/api/generated/model";
import { ROLE_DIFFERENCES } from "@/routes/overrides";
import { routeById } from "@/routes/table";
import { WEB } from "@/test/source-files";
import { ROUTES } from "@/routes/table";
import { NAV_GROUPS, formatBadge, navIdForRoute, visibleGroups, type NavEntry } from "./nav";

// The sidebar is driven by the route table: an item is shown when the role may open its route, and by nothing else, so the
// sidebar can never offer a link to a screen that would answer with the permission state, nor hide one that would open.
// Here it is also compared with the Shell frames of the handoff (Admin, and Viewer with destinations hidden).

const labels = (entries: readonly NavEntry[]): string[] => entries.map((e) => e.label);

describe("the role matrix", () => {
  it("shows an Admin everything, in the design's three groups", () => {
    const groups = visibleGroups(Role.admin);
    expect(groups.map(labels)).toEqual([["Swimlanes", "Compare", "Search"], ["Feature status", "Findings", "Approvals"], ["Audit", "Admin"]]);
  });

  it("shows a Viewer, an Editor and an Operator the same: no Approvals, no Audit, no Admin", () => {
    for (const role of [Role.viewer, Role.editor, Role.operator]) {
      expect(visibleGroups(role).map(labels), role).toEqual([["Swimlanes", "Compare", "Search"], ["Feature status", "Findings"]]);
    }
  });

  it("shows nobody with no role anything, and drops a group that is empty", () => {
    expect(visibleGroups(null)).toEqual([]);
    for (const role of Object.values(Role)) for (const group of visibleGroups(role)) expect(group.length).toBeGreaterThan(0);
  });

  it("offers an item to exactly the roles that may open its route", () => {
    const order = Object.values(Role);
    for (const entry of NAV_GROUPS.flat()) {
      const need = routeById(entry.route).access;
      for (const role of order) {
        const shown = visibleGroups(role).flat().some((e) => e.id === entry.id);
        expect(shown, `${entry.label} for ${role}`).toBe(need === "public" || order.indexOf(role) >= order.indexOf(need));
      }
    }
  });
});

describe("against the Shell frames of the handoff", () => {
  // Every `{ k: 'swimlanes', l: 'Swimlanes', i: 'layers', ..., gate: true }` of the Shell page's `G` array.
  const shell = readFileSync(join(WEB, "..", "design", "handoff", "screens", "Lanekeeper Shell.dc.html"), "utf8");
  const items = [...shell.matchAll(/\{ k: '([a-z-]+)', l: '([^']+)', i: '([a-z0-9-]+)'([^{}]*)\}/g)].map((m) => ({ key: m[1] ?? "", label: m[2] ?? "", gated: /gate: true/.test(m[4] ?? "") }));

  it("reads the Shell's eight items", () => {
    expect(items.map((i) => i.label)).toEqual(["Swimlanes", "Compare", "Search", "Feature status", "Findings", "Approvals", "Audit", "Admin"]);
  });

  it("shows an Admin all of them, in the Shell's order", () => {
    expect(labels(visibleGroups(Role.admin).flat())).toEqual(items.map((i) => i.label));
  });

  it("shows a Viewer what the Shell shows with destinations hidden, except where a decision says otherwise", () => {
    const frame = items.filter((i) => !i.gated).map((i) => i.label);
    const decided = ROLE_DIFFERENCES.filter((d) => d.chosen === Role.admin).map((d) => d.item);
    const viewer = labels(visibleGroups(Role.viewer).flat());
    const missing = frame.filter((l) => !viewer.includes(l));
    expect(missing).toEqual(["Audit"]);
    expect(decided).toContain("Audit");
    const extra = viewer.filter((l) => !frame.includes(l));
    expect(extra).toEqual([]);
  });
});

describe("the badges", () => {
  it("caps a count at 99", () => {
    expect(formatBadge(0)).toBe("0");
    expect(formatBadge(11)).toBe("11");
    expect(formatBadge(99)).toBe("99");
    expect(formatBadge(100)).toBe("99+");
    expect(formatBadge(4000)).toBe("99+");
    // A count the hub says is a floor (there are more pages) carries a plus, up to the cap.
    expect(formatBadge(25, true)).toBe("25+");
    expect(formatBadge(99, true)).toBe("99+");
    expect(formatBadge(100, true)).toBe("99+");
    expect(formatBadge(25, false)).toBe("25");
  });
});

describe("the current item", () => {
  it("is the item a screen belongs under: files under Swimlanes, a doc under Search, an approval under Approvals", () => {
    expect(navIdForRoute("files")).toBe("swimlanes");
    expect(navIdForRoute("swimlane-audit")).toBe("swimlanes");
    expect(navIdForRoute("doc")).toBe("search");
    expect(navIdForRoute("approval")).toBe("approvals");
    expect(navIdForRoute("admin-section")).toBe("admin");
  });

  it("is no item for the account and error screens, and every item it names is in the sidebar", () => {
    for (const id of ["github-token", "mcp-tokens", "onboarding", "not-found", "error", undefined]) expect(navIdForRoute(id), String(id)).toBeNull();
    const items = new Set(NAV_GROUPS.flat().map((e) => e.id));
    for (const route of ROUTES) {
      const item = navIdForRoute(route.id);
      if (item !== null) expect(items.has(item), `${route.id} names ${item}`).toBe(true);
    }
  });
});
