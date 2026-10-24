import { describe, expect, it } from "vitest";
import { buildRoutes, decodeCell, renderRoutesTs } from "./extract-routes.mjs";

describe("decodeCell", () => {
  it("drops the marker characters the Engineering Handoff page uses for styling", () => {
    expect(decodeCell("^Sign in")).toEqual({ text: "Sign in", mono: false });
    expect(decodeCell("`/signin")).toEqual({ text: "/signin", mono: true });
    expect(decodeCell("!Redesign")).toEqual({ text: "Redesign", mono: false });
    expect(decodeCell("_type-ahead")).toEqual({ text: "type-ahead", mono: false });
    expect(decodeCell("")).toEqual({ text: "", mono: false });
    expect(decodeCell({ t: "`x", m: false })).toEqual({ text: "x", mono: true });
  });
});

describe("extract-routes (the screens table)", () => {
  const data = buildRoutes();

  it("reads the 25 screen rows with route, query, role and source file", () => {
    expect(data.rows).toHaveLength(25);
    expect(data.rows[0]).toEqual({
      screen: "Sign in",
      route: "/signin",
      query: "next",
      role: "Anyone",
      source: "Access Screen",
      paths: ["/signin"],
    });
  });

  it("extracts every path shape, including the ones inside prose", () => {
    const byScreen = Object.fromEntries(data.rows.map((r) => [r.screen, r]));
    expect(byScreen["Files (tree and table)"].paths).toEqual(["/swimlanes/:id/files"]);
    expect(byScreen["Not found, error, forbidden"].paths).toEqual(["/404", "/error", "/403"]);
    expect(byScreen["Admin: paths, merge, tiers, folders, notify"].paths).toEqual(["/admin/:section"]);
    expect(byScreen["Command palette, Shortcuts sheet"].paths).toEqual([]);
    expect(byScreen["Doc reader"].paths).toEqual(["/docs/:slug"]);
  });

  it("keeps the note under the table", () => {
    expect(data.note).toMatch(/permission screen, not a redirect/);
  });

  it("renders a typed module", () => {
    expect(renderRoutesTs(data)).toContain("export const ROUTES = ");
  });
});
