import { describe, expect, it } from "vitest";
import { ROUTES as HANDOFF } from "@/handoff/routes.generated";
import { Role } from "@/api/generated/model";
import { hrefFor, LinkError } from "./links";
import { ADMIN_SECTIONS, paramCheck, paramNames, ROUTES, routeById } from "./table";

// The table is the reviewed copy of the handoff's screen inventory plus the approved additions. These tests compare it
// with the extracted handoff data (src/handoff/routes.generated.ts) so neither can drift without a person noticing.

const handoffPaths = [...new Set(HANDOFF.rows.flatMap((r) => r.paths))];

describe("route table against the handoff inventory", () => {
  it("has every route of the handoff, as an origin: handoff row", () => {
    const tablePaths = new Set(ROUTES.filter((r) => r.origin === "handoff").map((r) => r.path));
    for (const path of handoffPaths) expect(tablePaths.has(path), `handoff route ${path} is in the table`).toBe(true);
  });

  it("names no handoff route that the handoff does not have", () => {
    const expected = new Set<string>(handoffPaths);
    for (const route of ROUTES.filter((r) => r.origin === "handoff")) {
      if (route.id === "access-pending-alias") continue; // /access/pending is the handoff's own path, and it redirects to the server's
      expect(expected.has(route.path), `${route.path} is in the handoff`).toBe(true);
    }
  });

  it("agrees with the handoff on who may open a screen, except where the plan decided otherwise", () => {
    const decided = new Map<string, Role>([
      ["/swimlanes/:id/audit", Role.admin], // handoff: Viewer+. The contract's audit reads are Admin-only (W11).
      ["/audit", Role.admin], // handoff: Viewer+. Same reason (W11).
      ["/approvals", Role.admin], // handoff: Editor+. Approvals are Admin to open and decide (W12).
      ["/approvals/:id", Role.admin],
    ]);
    const fromHandoff = (text: string): Role | "public" => {
      if (/^Anyone|^No role/.test(text)) return "public";
      if (/^Admin/.test(text)) return Role.admin;
      if (/^Editor/.test(text)) return Role.editor;
      return Role.viewer;
    };
    const order: Array<Role | "public"> = ["public", Role.viewer, Role.editor, Role.operator, Role.admin];
    for (const route of ROUTES.filter((r) => r.origin === "handoff")) {
      const rows = HANDOFF.rows.filter((r) => r.route === route.path);
      if (rows.length === 0) continue; // /404 · /error · /403 is one handoff row, and the aliases have their own
      // A path on several handoff rows (Files, Diff, File viewer) takes the least restrictive of them.
      const lowest = rows.map((r) => fromHandoff(r.role)).reduce((a, b) => (order.indexOf(a) <= order.indexOf(b) ? a : b));
      expect(route.access, route.path).toBe(decided.get(route.path) ?? lowest);
    }
  });

  it("has unique ids and unique paths", () => {
    expect(new Set(ROUTES.map((r) => r.id)).size).toBe(ROUTES.length);
    expect(new Set(ROUTES.map((r) => r.path)).size).toBe(ROUTES.length);
  });

  it("makes every screen before the session public, and nothing else public but the error screens", () => {
    const publicIds = ROUTES.filter((r) => r.access === "public").map((r) => r.id).sort();
    expect(publicIds).toEqual(["access-disabled", "access-disabled-alias", "access-pending", "access-pending-alias", "error", "forbidden", "not-found", "signin"]);
  });

  it("reads only string keys it names, with no duplicate key in a route", () => {
    for (const route of ROUTES) {
      const keys = route.search ?? [];
      expect(new Set(keys).size, route.id).toBe(keys.length);
    }
  });

  it("keeps the admin sections the plan lists", () => {
    expect([...ADMIN_SECTIONS]).toEqual(["paths", "merge", "tiers", "folders", "notify", "mcp", "sentinels", "oslogin", "severity", "retention"]);
  });
});

describe("path parameters", () => {
  const swimlane = routeById("swimlane");
  it("accepts a swimlane id and nothing that could change the URL", () => {
    const ok = paramCheck(swimlane, "id");
    expect(ok?.("sit1")).toBe(true);
    for (const bad of ["", "SIT1", "../admin", "a/b", "a%2Fb", "-x", "a".repeat(64), "a b", "a.b"]) expect(ok?.(bad), bad).toBe(false);
  });
  it("accepts a numeric id for approvals, proposals and drafts", () => {
    for (const id of ["approval", "proposal-link", "draft"]) {
      const ok = paramCheck(routeById(id), "id");
      expect(ok?.("17")).toBe(true);
      for (const bad of ["0", "01", "-1", "1.5", "abc", "1".repeat(19), ""]) expect(ok?.(bad), `${id} ${bad}`).toBe(false);
    }
  });
  it("accepts an encoded doc slug without dots or slashes", () => {
    const ok = paramCheck(routeById("doc"), "slug");
    expect(ok?.("L2dpdC9tYWluL2RvY3MvYS5tZA")).toBe(true);
    for (const bad of ["a.md", "a/b", "", "a b", "x".repeat(2049)]) expect(ok?.(bad), bad).toBe(false);
  });
  it("accepts only the admin sections", () => {
    const ok = paramCheck(routeById("admin-section"), "section");
    expect(ok?.("paths")).toBe(true);
    expect(ok?.("users")).toBe(false);
    expect(ok?.("../x")).toBe(false);
  });
  it("lists the parameters of every path, and every one has a check", () => {
    for (const route of ROUTES) for (const name of paramNames(route.path)) expect(paramCheck(route, name), `${route.id} ${name}`).toBeTypeOf("function");
  });
});

describe("hrefFor", () => {
  it("builds a path and a search string", () => {
    expect(hrefFor("files", { id: "sit1" }, { path: "a/b.yml", line: "42" })).toBe("/swimlanes/sit1/files?line=42&path=a%2Fb.yml");
    expect(hrefFor("swimlanes")).toBe("/swimlanes");
    expect(hrefFor("admin-section", { section: "notify" })).toBe("/admin/notify");
  });
  it("refuses a parameter that is missing or not valid, and a search key the route does not read", () => {
    expect(() => hrefFor("swimlane")).toThrow(LinkError);
    expect(() => hrefFor("swimlane", { id: "../x" })).toThrow(LinkError);
    expect(() => hrefFor("swimlanes", {}, { nope: "1" })).toThrow(LinkError);
  });
  it("builds a link for every route that has no parameters", () => {
    for (const route of ROUTES.filter((r) => paramNames(r.path).length === 0)) expect(hrefFor(route.id)).toBe(route.path);
  });
});
