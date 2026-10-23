import { describe, expect, it } from "vitest";
import { ADMIN_SECTIONS } from "@/routes/table";
import { ADMIN_AREAS, ADMIN_NAV, areaCopy, areaHref, isAdminArea } from "./nav";

// The Admin area's own navigation (the left list of the design): four groups, the items of the Admin page, and where each one goes.

describe("the Admin navigation", () => {
  it("has the design's four groups in the design's order", () => {
    expect(ADMIN_NAV.map((g) => g.label)).toEqual(["Access", "Infrastructure", "Mapping and rules", "Data and alerts"]);
    expect(ADMIN_NAV.map((g) => g.items.map((i) => i.id))).toEqual([
      ["users"],
      ["swimlanes", "sentinels", "tiers"],
      ["paths", "merge", "severity", "folders", "oslogin"],
      ["retention", "notify", "mcp"],
    ]);
  });

  it("names every section the route accepts, and the two screens of their own", () => {
    const named = ADMIN_NAV.flatMap((g) => g.items.map((i) => i.id)).sort();
    expect(named).toEqual(["users", "swimlanes", ...ADMIN_SECTIONS].sort());
    expect([...ADMIN_AREAS].sort()).toEqual(named);
  });

  it("links the two screens of their own to their routes and the others to /admin/:section", () => {
    expect(areaHref("users")).toBe("/admin/users");
    expect(areaHref("swimlanes")).toBe("/admin/swimlanes");
    expect(areaHref("paths")).toBe("/admin/paths");
    expect(areaHref("oslogin")).toBe("/admin/oslogin");
  });

  it("has a title and a lead for every section, in the design's words", () => {
    for (const id of ADMIN_AREAS) {
      const copy = areaCopy(id);
      expect(copy.title.length, id).toBeGreaterThan(2);
      expect(copy.lead.length, id).toBeGreaterThan(10);
    }
    expect(areaCopy("users")).toEqual({ title: "Users and access requests", lead: "Approve new sign-ins, set roles, and choose whose changes need an Admin's approval." });
    expect(areaCopy("mcp").title).toBe("MCP personal tokens");
  });

  it("lists every section as one that is built: none is left for another task", () => {
    expect(ADMIN_NAV.flatMap((g) => g.items).filter((i) => "later" in i)).toEqual([]);
  });

  it("words the severity lead as the registry rates (the highest severity wins), not as the design did (the first rule wins)", () => {
    expect(areaCopy("severity").lead).toMatch(/highest severity wins/);
    expect(areaCopy("severity").lead).not.toMatch(/first matching rule/i);
  });

  it("tells a section from anything else", () => {
    expect(isAdminArea("tiers")).toBe(true);
    expect(isAdminArea("nope")).toBe(false);
  });
});
