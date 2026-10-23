import { describe, expect, it } from "vitest";
import { crumbsFor } from "./crumbs";

// The breadcrumb is a function of the route: where you are, and a link to each place above it.

describe("crumbsFor", () => {
  it("is the one page name on a top-level screen, and it is the current page (no link)", () => {
    for (const [id, label] of [["swimlanes", "Swimlanes"], ["compare", "Compare"], ["status", "Feature status"], ["findings", "Findings"], ["approvals", "Approvals"], ["audit", "Audit"]] as const) {
      expect(crumbsFor(id, {}, {}), id).toEqual([{ label }]);
    }
  });

  it("has the swimlane, then the tab, for a swimlane's screens", () => {
    expect(crumbsFor("swimlane", { id: "sitb" }, {})).toEqual([{ label: "Swimlanes", href: "/swimlanes" }, { label: "sitb", mono: true }]);
    expect(crumbsFor("services", { id: "sitb" }, {})).toEqual([{ label: "Swimlanes", href: "/swimlanes" }, { label: "sitb", mono: true, href: "/swimlanes/sitb" }, { label: "Services" }]);
    expect(crumbsFor("drift", { id: "sitb" }, {}).at(-1)).toEqual({ label: "Drift" });
    expect(crumbsFor("swimlane-findings", { id: "sitb" }, {}).at(-1)).toEqual({ label: "Findings" });
    expect(crumbsFor("adopt", { id: "sitb" }, {}).at(-1)).toEqual({ label: "Adopt this swimlane" });
    expect(crumbsFor("swimlane-pull-requests", { id: "sitb" }, {})).toEqual([{ label: "Swimlanes", href: "/swimlanes" }, { label: "sitb", mono: true, href: "/swimlanes/sitb" }, { label: "Pull requests" }]);
  });

  it("walks the path of a file: the swimlane, each folder, and the file as the current page", () => {
    expect(crumbsFor("files", { id: "sitb" }, { path: "tx-infinity-api/conf/tx-infinity-core-sit1.yml" })).toEqual([
      { label: "Swimlanes", href: "/swimlanes" },
      { label: "sitb", mono: true, href: "/swimlanes/sitb" },
      { label: "tx-infinity-api", mono: true, href: "/swimlanes/sitb/files?path=tx-infinity-api" },
      { label: "conf", mono: true, href: "/swimlanes/sitb/files?path=tx-infinity-api%2Fconf" },
      { label: "tx-infinity-core-sit1.yml", mono: true },
    ]);
  });

  it("says Files when no file is chosen", () => {
    expect(crumbsFor("files", { id: "sitb" }, {}).at(-1)).toEqual({ label: "Files" });
  });

  it("has Search, then the kind of search", () => {
    expect(crumbsFor("search", {}, { type: "settings" })).toEqual([{ label: "Search", href: "/search" }, { label: "Settings" }]);
    expect(crumbsFor("search", {}, {})).toEqual([{ label: "Search", href: "/search" }, { label: "Text" }]);
    expect(crumbsFor("search", {}, { type: "nonsense" }).at(-1)).toEqual({ label: "Text" });
  });

  it("has Admin, then the section's name", () => {
    expect(crumbsFor("admin-users", {}, {})).toEqual([{ label: "Admin", href: "/admin/users" }, { label: "Users and access requests" }]);
    expect(crumbsFor("admin-section", { section: "oslogin" }, {}).at(-1)).toEqual({ label: "OS Login user mapping" });
    expect(crumbsFor("admin-section", { section: "mcp" }, {}).at(-1)).toEqual({ label: "MCP personal tokens" });
  });

  it("has Approvals, then the proposal's number", () => {
    expect(crumbsFor("approval", { id: "17" }, {})).toEqual([{ label: "Approvals", href: "/approvals" }, { label: "#17" }]);
  });

  it("uses the route's own title for the account screens, and nothing for a route it does not know", () => {
    expect(crumbsFor("github-token", {}, {})).toEqual([{ label: "My GitHub token" }]);
    expect(crumbsFor("mcp-tokens", {}, {})).toEqual([{ label: "Personal MCP tokens" }]);
    expect(crumbsFor(undefined, {}, {})).toEqual([]);
    expect(crumbsFor("no-such-route", {}, {})).toEqual([]);
  });

  it("builds a link only from values the route table accepts: a swimlane id that is not valid gives no link", () => {
    const crumbs = crumbsFor("services", { id: "../x" }, {});
    expect(crumbs.every((c) => c.href === undefined || !c.href.includes(".."))).toBe(true);
    expect(crumbs.at(-1)).toEqual({ label: "Services" });
  });

  it("keeps a very long path bounded", () => {
    const path = Array.from({ length: 200 }, (_, i) => `d${i}`).join("/");
    expect(crumbsFor("files", { id: "sitb" }, { path }).length).toBeLessThanOrEqual(12);
  });
});
