import { describe, expect, it } from "vitest";
import type { DocHits, FeatureStatus, SettingHitPage, TextResults } from "../../../api/generated/model/index.ts";
import { contextFor, resolveBody, type MockContext, type MockResponse } from "../../scenario.ts";
import { scenarios } from "./index.ts";

// The scenarios of this slice reproduce the content of the Search and Feature Status pages of the handoff, so that the pixels of
// the app can be compared with the pixels of the design. The contract check is src/mocks/mocks.test.ts; this checks the content.

const byId = (id: string) => scenarios.find((s) => s.id === id);
function answer<T>(id: string, path: string, query: string, as: (body: unknown) => T): T;
function answer(id: string, path: string, query?: string): unknown;
function answer(id: string, path: string, query = "", as?: (body: unknown) => unknown): unknown {
  const response = byId(id)?.responses.find((r: MockResponse) => r.path === path && r.method === "GET");
  if (response === undefined) throw new Error(`${id} has no ${path}`);
  const context: MockContext = { ...contextFor(response), query: new URLSearchParams(query) };
  const body = resolveBody(response, context);
  return as === undefined ? body : as(body);
}
const asText = (body: unknown) => body as TextResults;
const asSettings = (body: unknown) => body as SettingHitPage;
const asDocs = (body: unknown) => body as DocHits;
const asFlags = (body: unknown) => body as FeatureStatus;

describe("the Search page data", () => {
  it("has the scenarios the board's frames need, named for this slice", () => {
    expect(scenarios.map((s) => s.id)).toEqual(["design-search-admin", "design-search-editor", "design-search-viewer", "design-search-truncated", "design-search-invalid", "design-search-empty", "design-search-error"]);
  });

  it("finds enableAccountSorting in the tenant and base file of each lane, as frame 1a shows", () => {
    const { hits, truncated } = answer("design-search-admin", "/api/v1/search/text", "pattern=enableAccountSorting", asText);
    expect(truncated).toBe(false);
    expect(hits).toHaveLength(23);
    expect(new Set(hits.map((h) => h.swimlane)).size).toBe(6);
    const sitb = hits.filter((h) => h.swimlane === "sitb");
    expect(sitb.map((h) => [h.path, h.line])).toEqual([
      ["tx-infinity-api/tx-infinity-core-sit7.yml", 13],
      ["tx-infinity-api/tx-infinity-core.yml", 13],
      ["tx-infinity-api/tx-infinity-core.yml", 27],
      ["tx-infinity-api/tx-infinity-gateway.yml", 41],
    ]);
    expect(sitb[0]?.tenant).toBe("sit7");
    expect(sitb[1]?.tenant).toBeUndefined();
  });

  it("marks the match with byte offsets of the line", () => {
    const { hits } = answer("design-search-admin", "/api/v1/search/text", "pattern=enableAccountSorting", asText);
    for (const hit of hits) {
      const [start, end] = hit.ranges[0] ?? [0, 0];
      expect(new TextDecoder().decode(new TextEncoder().encode(hit.text).slice(start, end)).toLowerCase()).toBe("enableaccountsorting");
    }
  });

  it("narrows by swimlane, by case and by path glob", () => {
    expect(answer("design-search-admin", "/api/v1/search/text", "pattern=enableAccountSorting&swimlanes=presita,presitc", asText).hits.map((h) => h.swimlane)).not.toContain("sit");
    expect(answer("design-search-admin", "/api/v1/search/text", "pattern=ENABLEACCOUNTSORTING&case_sensitive=true", asText).hits).toHaveLength(0);
    expect(answer("design-search-admin", "/api/v1/search/text", "pattern=enableAccountSorting&path_glob=**/*-sit7.yml", asText).hits).toHaveLength(1);
  });

  it("caps a truncated search at 500 hits and says so (frame 1c)", () => {
    const body = answer("design-search-truncated", "/api/v1/search/text", "pattern=timeout", asText);
    expect(body.hits).toHaveLength(500);
    expect(body.truncated).toBe(true);
  });

  it("finds a setting by a part of its path, in the tenant file and the base file (frame 1g)", () => {
    const { items } = answer("design-search-admin", "/api/v1/search/settings", "q=enableAccountSorting", asSettings);
    expect(items).toHaveLength(17);
    expect(items.every((i) => i.path.toLowerCase().includes("enableaccountsorting"))).toBe(true);
  });

  it("finds the sections of the docs for the words of a query (frame 1j), with the heading the server names", () => {
    const { hits } = answer("design-search-admin", "/api/v1/docs/search", "q=account sorting&limit=50", asDocs);
    expect(hits).toHaveLength(5);
    expect(hits[1]).toMatchObject({ title: "Account sorting", heading_path: ["Account sorting", "Feature Flags"], anchor: "doc-feature-flags" });
    expect(answer("design-search-admin", "/api/v1/docs/search", "q=ledger sorting", asDocs).hits).toHaveLength(0);
  });

  it("has a Git doc and an uploaded one, and renders both with heading ids the server chose", () => {
    const docs = answer("design-search-admin", "/api/v1/docs", "limit=100", (b) => b as { items: Array<{ source: string; title: string }> });
    expect(docs.items.map((d) => [d.title, d.source])).toEqual([
      ["Account sorting", "git"],
      ["Instant payments", "upload"],
      ["Receipts v3", "git"],
    ]);
    const page = answer("design-search-admin", "/api/v1/docs/fs/render", "path=/uploads/instant-payments.md", (b) => b as { headings: Array<{ id: string }>; html: string });
    expect(page.headings.every((h) => /^doc-[a-z0-9]+(-[a-z0-9]+)*$/.test(h.id))).toBe(true);
    for (const heading of page.headings) expect(page.html).toContain(`id="${heading.id}"`);
  });
});

describe("the Feature Status page data", () => {
  it("has the flags of the page: eight flags, two of them in two channels, and one that does not differ", () => {
    const { flags } = answer("design-search-admin", "/api/v1/features/status", "", asFlags);
    expect(new Set(flags.map((f) => f.flag)).size).toBe(8);
    expect(flags.filter((f) => f.channel !== null).length).toBeGreaterThanOrEqual(6);
    expect(flags.every((f) => f.values.length === 6)).toBe(true);
  });

  it("has an empty list for the empty state (States E06)", () => {
    expect(answer("design-search-empty", "/api/v1/features/status", "", asFlags).flags).toEqual([]);
  });
});

describe("the states", () => {
  it("answers 500 on every read of these screens (States R05, R06)", () => {
    const failing = byId("design-search-error")?.responses.filter((r) => r.status === 500).map((r) => r.path);
    expect(failing).toEqual(expect.arrayContaining(["/api/v1/search/text", "/api/v1/search/settings", "/api/v1/docs/search", "/api/v1/docs", "/api/v1/docs/fs/render", "/api/v1/features/status"]));
  });

  it("refuses the pattern with a 400 (frame 1d)", () => {
    expect(byId("design-search-invalid")?.responses.find((r) => r.path === "/api/v1/search/text")?.status).toBe(400);
  });

  it("has an Editor, a Viewer and an Admin", () => {
    const role = (id: string) => answer(id, "/api/v1/me", "", (b) => b as { role: string }).role;
    expect([role("design-search-admin"), role("design-search-editor"), role("design-search-viewer")]).toEqual(["admin", "editor", "viewer"]);
  });
});
