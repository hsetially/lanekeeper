import { describe, expect, it } from "vitest";
import { buildPrIndex, prBadgeFor } from "../../../features/prs/badges";
import { contextFor, resolveBody, type MockContext, type MockResponse } from "../../scenario";
import { scenarios as swimlaneScenarios } from "../swimlanes/index";
import { scenarios } from "./index";
import { LINKS } from "./fixtures";

// The scenarios of the pull-request pieces. `src/mocks/mocks.test.ts` checks every response against the OpenAPI schema; these
// check that the data says what the design's frames say, and that it agrees with the scenarios of the screens it decorates.

const byId = (id: string) => scenarios.find((s) => s.id === id);
const prsOf = (id: string): MockResponse => {
  const found = byId(id)?.responses.find((r) => r.method === "GET" && r.path === "/api/v1/prs");
  if (!found) throw new Error(`${id} answers no pull request list`);
  return found;
};
const ask = (id: string, query: string) => resolveBody(prsOf(id), { ...contextFor(prsOf(id)), query: new URLSearchParams(query) } satisfies MockContext) as { items: Array<{ id: number; swimlane: string; state: string; pr_number: number; path: string }>; next: string | null };

describe("the ids", () => {
  it("are design-prs-<name>", () => {
    expect(scenarios.length).toBeGreaterThanOrEqual(4);
    for (const s of scenarios) expect(s.id).toMatch(/^design-prs-[a-z-]+$/);
  });
});

describe("design-prs-list", () => {
  it("filters the way the hub does: by swimlane and by state, newest first", () => {
    const open = ask("design-prs-list", "swimlane=sitb&state=open&limit=100");
    expect(open.items.length).toBeGreaterThan(1);
    expect(open.items.every((l) => l.swimlane === "sitb" && l.state === "open")).toBe(true);
    expect(open.items.map((l) => l.id)).toEqual([...open.items.map((l) => l.id)].sort((a, b) => b - a));
    const all = ask("design-prs-list", "swimlane=sitb&limit=100");
    expect(new Set(all.items.map((l) => l.state))).toEqual(new Set(["open", "merged", "closed"]));
    expect(all.items.some((l) => l.swimlane !== "sitb")).toBe(false);
  });

  it("has a pull request that carries two files, so that it makes two rows with one number", () => {
    const all = ask("design-prs-list", "swimlane=sitb&limit=100");
    const counts = new Map<number, number>();
    for (const l of all.items) counts.set(l.pr_number, (counts.get(l.pr_number) ?? 0) + 1);
    expect([...counts.values()].some((n) => n >= 2)).toBe(true);
  });

  it("is a single page", () => {
    expect(ask("design-prs-list", "swimlane=sitb&limit=100").next).toBeNull();
  });
});

describe("design-prs-empty", () => {
  it("has no link at all", () => {
    expect(ask("design-prs-empty", "swimlane=sitb&limit=100").items).toEqual([]);
  });
});

describe("design-prs-drift-badges (frame 1l)", () => {
  const drift = swimlaneScenarios.find((s) => s.id === "design-swimlanes-six")?.responses.find((r) => r.path === "/api/v1/swimlanes/{id}/drift");
  const files = (drift === undefined ? { items: [] } : resolveBody(drift, { ...contextFor(drift), params: { id: "sitb" } })) as { items: Array<{ path: string; hash: string | null; drift: "nfs_ahead" }> };
  const index = buildPrIndex("sitb", ask("design-prs-drift-badges", "swimlane=sitb&state=open&limit=100").items as never[]);
  const merged = buildPrIndex("sitb", ask("design-prs-drift-badges", "swimlane=sitb&state=merged&limit=100").items as never[]);

  it("gives the Drift tab of sitb the answers of the swimlanes scenario it decorates", () => {
    expect(byId("design-prs-drift-badges")?.responses.some((r) => r.path === "/api/v1/swimlanes/{id}/drift")).toBe(true);
    expect(files.items.length).toBeGreaterThan(0);
  });

  it("makes PR #418 open the badge of the tenant file, and Fix merged, awaiting sync the badge of application.properties", () => {
    const core = files.items.find((f) => f.path === "tx-infinity-api/tx-infinity-core-sit7.yml");
    const props = files.items.find((f) => f.path === "application.properties");
    expect(core).toBeDefined();
    expect(props).toBeDefined();
    expect(prBadgeFor(index, { path: core?.path ?? "", hash: core?.hash, drift: core?.drift })).toMatchObject({ kind: "open", outdated: false, link: { pr_number: 418 } });
    expect(prBadgeFor(merged, { path: props?.path ?? "", hash: props?.hash, drift: props?.drift })).toMatchObject({ kind: "merged", link: { pr_number: 417 } });
  });

  it("leaves a file that has no pull request without a badge", () => {
    const other = files.items.find((f) => f.path === "ui/remote-itm-teller/teller-ui.yml");
    expect(other).toBeDefined();
    expect(prBadgeFor(index, { path: other?.path ?? "", hash: other?.hash, drift: other?.drift })).toBeNull();
    expect(prBadgeFor(merged, { path: other?.path ?? "", hash: other?.hash, drift: other?.drift })).toBeNull();
  });
});

describe("design-prs-hostile", () => {
  it("has a path with a hidden control character, an address that is not https and an address with credentials", () => {
    const items = ask("design-prs-hostile", "swimlane=sitb&limit=100").items as Array<{ path: string; pr_url?: string }>;
    expect(items.some((l) => l.path.includes("‮"))).toBe(true);
    expect(items.some((l) => l.pr_url?.startsWith("javascript:") === true)).toBe(true);
    expect(items.some((l) => /^https:\/\/[^/]*@/.test(l.pr_url ?? ""))).toBe(true);
  });
});

describe("the links", () => {
  it("are whole: a number, an address with that number, an author and a time", () => {
    for (const l of LINKS) {
      expect(l.pr_url).toContain(`/pull/${l.pr_number}`);
      expect(l.author).toMatch(/^[0-9a-f-]{36}:[0-9a-f-]{36}$/);
      expect(l.closed_at === null).toBe(l.state === "open");
    }
  });
});
