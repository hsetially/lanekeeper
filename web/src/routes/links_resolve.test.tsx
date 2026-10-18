import { readFileSync } from "node:fs";
import { join } from "node:path";
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { json, me, renderApp } from "@/test/app";
import { WEB } from "@/test/source-files";
import { PAGE_LOADERS, type PageLoaders } from "./pages";

// The MCP tools and the Teams cards send people to URLs that the hub composes (`LinkTemplates::default()` in
// crates/hub-mcp/src/output/links.rs). Every one of those shapes has to land on a screen of the app and not on the 404 page, and
// the keys that say WHAT to show have to reach the screen that shows it. The templates are read back from the Rust source, so a
// template added there fails here until it is given a place; and the shapes whose screens are later tasks are listed with their
// reason, and are checked to still be unrouted, so that the list cannot go stale.

const source = readFileSync(join(WEB, "..", "crates/hub-mcp/src/output/links.rs"), "utf8");
const block = /impl Default for LinkTemplates \{[\s\S]*?\n {4}\}\n\}/.exec(source)?.[0] ?? "";
const templates: Record<string, string> = Object.fromEntries([...block.matchAll(/(\w+): t\("([^"]+)"\)/g)].map((m) => [m[1] ?? "", m[2] ?? ""]));

const SAMPLE: Record<string, string> = {
  swimlane: "sitb",
  path: encodeURIComponent("core/tx-infinity-core-sit7.yml"),
  left: encodeURIComponent("nfs:sitb"),
  right: encodeURIComponent("git:base@main"),
  file: encodeURIComponent("core/tx-infinity-core.yml"),
  swimlanes: encodeURIComponent("sitb,sitc"),
  q: encodeURIComponent("server.port"),
  tenant: "sit7",
  application: "payments",
  id: "12",
};
const filled = (template: string): string => template.replace(/\{(\w+)\}/g, (_m, name: string) => SAMPLE[name] ?? `MISSING-${name}`);

/** Where each template ends up: the route that shows it, the address it ends at, and the search keys that screen is given. */
const EXPECTED: Record<string, { route: string; href: string; search: Record<string, string> }> = {
  swimlane: { route: "swimlane", href: "/swimlanes/sitb", search: {} },
  file: { route: "files", href: "/swimlanes/sitb/files?path=core%2Ftx-infinity-core-sit7.yml", search: { path: "core/tx-infinity-core-sit7.yml" } },
  drift: { route: "drift", href: "/swimlanes/sitb/drift", search: {} },
  findings: { route: "findings", href: "/findings?swimlane=sitb", search: { swimlane: "sitb" } },
  restarts: { route: "services", href: "/swimlanes/sitb/services", search: {} },
  history: { route: "files", href: "/swimlanes/sitb/files?panel=history&path=core%2Ftx-infinity-core-sit7.yml", search: { panel: "history", path: "core/tx-infinity-core-sit7.yml" } },
  compare: {
    route: "compare",
    href: "/compare?file=core%2Ftx-infinity-core.yml&left=nfs%3Asitb&right=git%3Abase%40main",
    search: { file: "core/tx-infinity-core.yml", left: "nfs:sitb", right: "git:base@main" },
  },
  compare_tree: { route: "compare", href: "/compare?left=nfs%3Asitb&right=git%3Abase%40main", search: { left: "nfs:sitb", right: "git:base@main" } },
  grid: { route: "compare", href: "/compare?file=core%2Ftx-infinity-core.yml&lanes=sitb%2Csitc", search: { file: "core/tx-infinity-core.yml", lanes: "sitb,sitc" } },
  search_text: { route: "search", href: "/search?mode=text&q=server.port", search: { mode: "text", q: "server.port" } },
  search_settings: { route: "search", href: "/search?mode=settings&q=server.port", search: { mode: "settings", q: "server.port" } },
  docs_search: { route: "search", href: "/search?q=server.port&type=docs", search: { q: "server.port", type: "docs" } },
  doc: { route: "doc-view", href: "/docs/view?path=core%2Ftx-infinity-core-sit7.yml", search: { path: "core/tx-infinity-core-sit7.yml" } },
  draft: { route: "draft", href: "/drafts/12", search: {} },
};

/** Shapes whose screen is a later task. They show the 404 page for now, and the test says so until the screen exists. */
const LATER: Record<string, string> = {
  effective: "the effective-config view of a file in a swimlane: needs the rendered text of CG8 (T6)",
  served: "the As served view: needs the served-file comparison of CG8 (T6)",
  pr: "a swimlane's pull requests: the PR badges and list are T5",
};

// A page that shows which route it is and the search keys the router gave it.
const pages: PageLoaders = Object.fromEntries(
  Object.keys(PAGE_LOADERS).map((id) => [id, () => Promise.resolve({ default: ({ search }: { search: Record<string, string | undefined> }) => <h1 data-route={id} data-search={JSON.stringify(search)}>{id}</h1> })]),
);
const admin = (call: { path: string }) => (call.path === "/api/v1/me" ? json(me("admin")) : json({ items: [], next: null }));

describe("the links the MCP tools send", () => {
  it("are read from the hub's source, and every one has a place here or a reason it has none", () => {
    expect(Object.keys(templates).length).toBeGreaterThanOrEqual(17);
    expect(Object.keys(templates).sort()).toEqual([...Object.keys(EXPECTED), ...Object.keys(LATER)].sort());
  });

  it.each(Object.entries(EXPECTED))("%s lands on its screen with the keys that say what to show", async (name, want) => {
    const template = templates[name] ?? "";
    const app = renderApp(filled(template), admin, { pages });
    const heading = await screen.findByRole("heading", { level: 1, name: want.route });
    expect(heading).toHaveAttribute("data-route", want.route);
    await waitFor(() => expect(app.location()).toBe(want.href));
    expect(JSON.parse(heading.getAttribute("data-search") ?? "null")).toEqual(want.search);
  });

  it.each(Object.entries(LATER))("%s is not routed yet (%s)", async (name) => {
    renderApp(filled(templates[name] ?? ""), admin, { pages });
    expect(await screen.findByRole("heading", { level: 1, name: "Page not found" })).toBeInTheDocument();
  });
});
