import { describe, expect, it } from "vitest";
import { Role, type DocHit, type SettingHit, type SwimlaneSummary, type User } from "@/api/generated/model";
import { ACTIONS, PALETTE_GROUPS, buildPalette, type PaletteInput, type Remote } from "./model";

// What the palette shows, as data: which groups a role gets, what a query narrows them to, what ⌘1 to ⌘9 mean (the position in
// the visible list), and that nothing is offered that the role cannot open or that would be a broken link.

const swimlane = (id: string, over: Partial<SwimlaneSummary> = {}): SwimlaneSummary =>
  ({ id, display_name: id, tenants: ["sit1"], agent: "online", last_scan_at: null, base_version: null, drift_counts: [], finding_count: 0, tier: "SIT", ...over }) as SwimlaneSummary;

const doc = (over: Partial<DocHit> = {}): DocHit => ({ doc: 1, title: "Account sorting", path: "remote-itm-teller.md", heading_path: ["Teller", "Sorting"], snippet: "x", score: 1, related_files: [], ...over });
const setting = (over: Partial<SettingHit> = {}): SettingHit =>
  ({ swimlane: "sitb", file: "tx-infinity-api/tx-infinity-core-sit1.yml", path: "txInfinityOptions.enableAccountSorting", value: { text: "true" }, location: { path: "a", lines: { start: 13, end: 13 } }, ...over }) as SettingHit;
const user = (over: Partial<User> = {}): User => ({ id: "u1:u2", email: "marco@example.test", display_name: "Marco Diaz", role: Role.editor, status: "active", requires_approval: false, github_login: null, last_seen_at: null, ...over });

const idle = { status: "idle", items: [] } as const;
const ready = <T,>(items: T[]): Remote<T> => ({ status: "ready", items });
const input = (over: Partial<PaletteInput> = {}): PaletteInput => ({
  query: "",
  role: Role.viewer,
  swimlanes: [swimlane("sit"), swimlane("sitb"), swimlane("presita", { tier: "PRESIT" })],
  docs: idle,
  settings: idle,
  users: idle,
  ...over,
});

const labels = (items: readonly { label: string }[]): string[] => items.map((i) => i.label);

describe("the table", () => {
  it("is the plan's: group, operation, minimum role, debounce, cap, cache", () => {
    expect(PALETTE_GROUPS.map((g) => [g.id, g.operation, g.minRole, g.debounceMs, g.cap])).toEqual([
      ["swimlanes", "listSwimlanes", Role.viewer, 0, 8],
      ["settings", "searchSettings", Role.viewer, 200, 8],
      ["docs", "searchDocs", Role.viewer, 200, 8],
      ["users", "adminListUsers", Role.admin, 200, 8],
      ["actions", "static", Role.viewer, 0, 8],
    ]);
    for (const g of PALETTE_GROUPS) expect(g.cache.length, g.id).toBeGreaterThan(0);
  });
});

describe("groups by role", () => {
  it("shows a Viewer swimlanes and actions on an empty query, and no Users group", () => {
    const view = buildPalette(input());
    expect(view.sections.map((s) => s.spec.id)).toEqual(["swimlanes", "actions"]);
    expect(labels(view.sections[0]?.items ?? [])).toEqual(["presita", "sit", "sitb"]);
  });

  it("gives Users to an Admin only, and only from two characters on", () => {
    const users = ready([user()]);
    expect(buildPalette(input({ role: Role.viewer, query: "marco", users })).sections.map((s) => s.spec.id)).not.toContain("users");
    expect(buildPalette(input({ role: Role.admin, query: "m", users })).sections.map((s) => s.spec.id)).not.toContain("users");
    const admin = buildPalette(input({ role: Role.admin, query: "marco", users }));
    expect(admin.sections.map((s) => s.spec.id)).toContain("users");
    expect(admin.items.find((i) => i.id.startsWith("users:"))?.action).toEqual({ kind: "go", href: "/admin/users?q=marco%40example.test" });
  });

  it("offers an action only if the role may open where it goes", () => {
    const names = (role: Role) => labels(buildPalette(input({ role })).sections.find((s) => s.spec.id === "actions")?.items ?? []);
    expect(names(Role.viewer)).not.toContain("Go to Audit");
    expect(names(Role.viewer)).not.toContain("Go to Approvals");
    expect(names(Role.viewer)).not.toContain("My GitHub token");
    expect(names(Role.editor)).toContain("My GitHub token");
    for (const label of ["Go to Audit", "Go to Approvals", "Go to Admin"]) expect(names(Role.admin)).toContain(label);
  });

  it("offers nothing to a person with no role", () => {
    expect(buildPalette(input({ role: null })).items).toEqual([]);
  });

  it("every action that goes somewhere names a route the table has", () => {
    for (const action of ACTIONS) if (action.run.kind === "go") expect(action.run.href, action.label).toMatch(/^\//);
  });
});

describe("a query", () => {
  it("narrows swimlanes and actions, best match first", () => {
    const view = buildPalette(input({ query: "sit" }));
    expect(labels(view.sections.find((s) => s.spec.id === "swimlanes")?.items ?? [])).toEqual(["sit", "sitb", "presita"]);
  });

  it("shows remote results in their own groups, with the plan's cap of 8", () => {
    const docs = ready(Array.from({ length: 20 }, (_, i) => doc({ doc: i, title: `Doc ${i}`, path: `d${i}.md` })));
    const view = buildPalette(input({ query: "doc", docs }));
    expect(view.sections.find((s) => s.spec.id === "docs")?.items).toHaveLength(8);
  });

  it("caps swimlanes at 8 too", () => {
    const many = Array.from({ length: 30 }, (_, i) => swimlane(`sit${String(i).padStart(2, "0")}`));
    expect(buildPalette(input({ swimlanes: many })).sections[0]?.items).toHaveLength(8);
  });

  it("says a group is loading or failed instead of hiding it", () => {
    const view = buildPalette(input({ query: "core", docs: { status: "loading", items: [] }, settings: { status: "error", items: [] } }));
    expect(view.sections.find((s) => s.spec.id === "docs")?.status).toBe("loading");
    expect(view.sections.find((s) => s.spec.id === "settings")?.status).toBe("error");
  });

  it("makes the visible list the order of the sections, and numbers the results (not the actions) for ⌘1 to ⌘9", () => {
    const view = buildPalette(input({ query: "core", docs: ready([doc()]), settings: ready([setting()]) }));
    expect(view.items.map((i) => i.id)).toEqual(view.sections.flatMap((s) => s.items.map((i) => i.id)));
    expect(new Set(view.items.map((i) => i.id)).size).toBe(view.items.length);
    expect(view.numbered.every((i) => !i.id.startsWith("actions:"))).toBe(true);
    expect(view.numbered.map((i) => i.id)).toEqual(view.items.filter((i) => !i.id.startsWith("actions:")).map((i) => i.id));
  });

  it("numbers at most nine results", () => {
    const docs = ready(Array.from({ length: 8 }, (_, i) => doc({ doc: i, title: `Doc ${i}`, path: `d${i}.md` })));
    const settings = ready(Array.from({ length: 8 }, (_, i) => setting({ path: `s.${i}` })));
    const view = buildPalette(input({ query: "doc", docs, settings, swimlanes: [swimlane("doc1"), swimlane("doc2")] }));
    expect(view.numbered).toHaveLength(9);
  });

  it("numbers nothing in actions-only mode", () => {
    expect(buildPalette(input({ query: ">go" })).numbered).toEqual([]);
  });

  it("has an empty view for a query that matches nothing and is not loading", () => {
    const view = buildPalette(input({ query: "qqqqqq" }));
    expect(view.items).toEqual([]);
  });
});

describe("the > prefix", () => {
  it("shows actions only, and matches the rest of the text", () => {
    const view = buildPalette(input({ query: ">shortcuts", role: Role.admin, docs: ready([doc()]) }));
    expect(view.actionsOnly).toBe(true);
    expect(view.sections.map((s) => s.spec.id)).toEqual(["actions"]);
    expect(labels(view.items)).toEqual(["Keyboard shortcuts"]);
  });

  it("with nothing after it lists every action the role has", () => {
    expect(buildPalette(input({ query: ">" })).items.length).toBeGreaterThan(5);
  });
});

describe("what a result opens", () => {
  it("opens a swimlane at its overview, and drops one whose id the router would not accept", () => {
    const view = buildPalette(input({ swimlanes: [swimlane("sitb"), swimlane("../x")] }));
    expect(view.items.find((i) => i.id === "swimlanes:sitb")?.action).toEqual({ kind: "go", href: "/swimlanes/sitb", swimlane: "sitb" });
    expect(view.items.find((i) => i.label === "../x")).toBeUndefined();
  });

  it("opens a setting in its file at its line", () => {
    const view = buildPalette(input({ query: "sorting", settings: ready([setting()]) }));
    const item = view.items.find((i) => i.id.startsWith("settings:"));
    expect(item?.action).toEqual({ kind: "go", href: "/swimlanes/sitb/files?line=13&path=tx-infinity-api%2Ftx-infinity-core-sit1.yml" });
    expect(item?.mono).toBe(true);
  });

  it("does not show a setting's value (it may be a secret the server did not recognise)", () => {
    const view = buildPalette(input({ query: "sorting", settings: ready([setting({ value: { text: "hunter2" } } as Partial<SettingHit>)]) }));
    expect(JSON.stringify(view.items)).not.toContain("hunter2");
  });

  it("opens a doc by its path, at the heading the hit came from only when the anchor is a plain id", () => {
    const withAnchor = buildPalette(input({ query: "sorting", docs: ready([doc({ anchor: "sorting-1" })]) })).items.find((i) => i.id.startsWith("docs:"));
    expect(withAnchor?.action).toEqual({ kind: "go", href: "/docs/view?path=remote-itm-teller.md#sorting-1" });
    const bad = buildPalette(input({ query: "sorting", docs: ready([doc({ anchor: "x\" onclick=\"y" })]) })).items.find((i) => i.id.startsWith("docs:"));
    expect(bad?.action).toEqual({ kind: "go", href: "/docs/view?path=remote-itm-teller.md" });
  });
});
